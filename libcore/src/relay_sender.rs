use super::*;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::oneshot;
use tokio::time::{timeout_at, Instant};

enum Outbound {
    Control(ClientToRelay),
    Deposit {
        to: [u8; 32],
        blob: Vec<u8>,
        deadline: Instant,
        complete: oneshot::Sender<Result<()>>,
    },
}

/// Clones share one framed writer. Only the terminal Deposited frame releases
/// a successful deposit to its caller, regardless of concurrent control sends.
#[derive(Clone)]
pub struct RelaySender {
    channel: mpsc::Sender<Outbound>,
    owner: [u8; 32],
    address: Arc<str>,
}

impl RelaySender {
    pub fn belongs_to(&self, peer: &[u8; 32]) -> bool {
        self.owner == *peer
    }

    pub fn terminal_address(&self) -> &str {
        &self.address
    }

    pub fn is_closed(&self) -> bool {
        self.channel.is_closed()
    }

    pub async fn send(&self, frame: ClientToRelay) -> Result<()> {
        match frame {
            ClientToRelay::Send { to, blob } => self.deposit(to, blob, None).await,
            other => self
                .channel
                .send(Outbound::Control(other))
                .await
                .map_err(|_| RelayError::Closed),
        }
    }

    pub async fn deposit(
        &self,
        to: [u8; 32],
        blob: Vec<u8>,
        remaining: Option<std::time::Duration>,
    ) -> Result<()> {
        if !self.belongs_to(&to) {
            return Err(RelayError::Proto(
                "terminal belongs to another identity".into(),
            ));
        }
        let limit = crate::relay_deposit::TIME_LIMIT;
        let deadline = Instant::now() + remaining.unwrap_or(limit).min(limit);
        let (complete, result) = oneshot::channel();
        timeout_at(deadline, async {
            self.channel
                .send(Outbound::Deposit {
                    to,
                    blob,
                    deadline,
                    complete,
                })
                .await
                .map_err(|_| RelayError::Closed)?;
            result.await.map_err(|_| RelayError::Closed)?
        })
        .await
        .map_err(|_| RelayError::Proto("fragmented deposit timed out".into()))?
    }
}

async fn deposit<W: AsyncWrite + Unpin>(
    wr: &mut W,
    replies: &mut mpsc::Receiver<RelayToClient>,
    to: [u8; 32],
    blob: &[u8],
) -> Result<()> {
    let parts = crate::relay_deposit::parts(blob).map_err(|e| RelayError::Proto(e.into()))?;
    let total = parts.len() as u16;
    // Pipeline parts on the ordered stream; no extra round-trip per audio or
    // file fragment. Validate every index and the final deposit confirmation.
    for (index, bytes) in parts.enumerate() {
        send(
            wr,
            &ClientToRelay::SendPartV1 {
                to,
                index: index as u16,
                total,
                bytes: bytes.to_vec(),
            },
        )
        .await?;
    }
    for expected in 0..total {
        match replies.recv().await.ok_or(RelayError::Closed)? {
            RelayToClient::SendPartAckV1 { index } if expected + 1 < total && index == expected => {
            }
            RelayToClient::Deposited { id } if expected + 1 == total && id != 0 => {}
            RelayToClient::Error(reason) => return Err(RelayError::Proto(reason)),
            _ => {
                return Err(RelayError::Proto(
                    "invalid fragmented deposit acknowledgement".into(),
                ))
            }
        }
    }
    Ok(())
}

pub(crate) fn start(
    stream: std::pin::Pin<Box<dyn crate::net::DuplexStream>>,
    owner: [u8; 32],
    address: &str,
) -> RelayClient {
    let (in_tx, in_rx) = mpsc::channel(256);
    let (out_tx, mut out_rx) = mpsc::channel(256);
    let (ack_tx, mut ack_rx) = mpsc::channel(32);
    let (mut rd, mut wr) = tokio::io::split(stream);
    let waiting = Arc::new(AtomicBool::new(false));
    let receiving = waiting.clone();
    tokio::spawn(async move {
        let mut incoming = crate::relay_deposit::Assembly::default();
        let mut message = None;
        loop {
            let reading = recv::<_, RelayToClient>(&mut rd);
            let frame = match incoming.deadline() {
                Some(end) => match timeout_at(end.into(), reading).await {
                    Ok(result) => result,
                    Err(_) => break,
                },
                None => reading.await,
            };
            let Ok(frame) = frame else { break };
            let frame = match frame {
                RelayToClient::IncomingPartV1 {
                    id,
                    from,
                    index,
                    total,
                    bytes,
                } if id > 0 && message.is_none_or(|expected| expected == id) => {
                    message = Some(id);
                    match incoming.receive_push(
                        from,
                        index,
                        total,
                        &bytes,
                        std::time::Instant::now(),
                    ) {
                        Ok(Some(blob)) => {
                            message = None;
                            RelayToClient::Incoming { id, from, blob }
                        }
                        Ok(None) => continue,
                        Err(_) => break,
                    }
                }
                RelayToClient::IncomingPartV1 { .. } => break,
                RelayToClient::Incoming { .. } => break,
                _ if message.is_some() => break,
                other => other,
            };
            crate::debug_trace!("[relay-wire] recv {}", frame_kind(&frame));
            if matches!(
                frame,
                RelayToClient::Deposited { .. } | RelayToClient::SendPartAckV1 { .. }
            ) || (matches!(frame, RelayToClient::Error(_)) && receiving.load(Ordering::Acquire))
            {
                if !receiving.load(Ordering::Acquire) || ack_tx.try_send(frame).is_err() {
                    break;
                }
            } else if in_tx.send(frame).await.is_err() {
                break;
            }
        }
    });
    tokio::spawn(async move {
        while let Some(work) = out_rx.recv().await {
            match work {
                Outbound::Control(frame) => {
                    crate::debug_trace!("[relay-wire] send {}", out_kind(&frame));
                    if !matches!(
                        tokio::time::timeout(
                            crate::relay_deposit::TIME_LIMIT,
                            send(&mut wr, &frame)
                        )
                        .await,
                        Ok(Ok(()))
                    ) {
                        break;
                    }
                }
                Outbound::Deposit {
                    to,
                    blob,
                    deadline,
                    complete,
                } => {
                    let now = Instant::now();
                    if complete.is_closed() || now >= deadline {
                        let _ = complete.send(Err(RelayError::Proto(
                            "fragmented deposit expired before transmission".into(),
                        )));
                        continue;
                    }
                    waiting.store(true, Ordering::Release);
                    let result = timeout_at(deadline, deposit(&mut wr, &mut ack_rx, to, &blob))
                        .await
                        .unwrap_or_else(|_| {
                            Err(RelayError::Proto("fragmented deposit timed out".into()))
                        });
                    waiting.store(false, Ordering::Release);
                    let closed = result.is_err();
                    let _ = complete.send(result);
                    // Late acknowledgements on a failed stream cannot satisfy a later upload.
                    if closed {
                        break;
                    }
                }
            }
        }
        let _ = wr.shutdown().await;
    });
    RelayClient {
        out_tx: RelaySender {
            channel: out_tx,
            owner,
            address: address.into(),
        },
        in_rx: Arc::new(Mutex::new(in_rx)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stream_for_another_owner_does_not_upload_mail() {
        let (client, mut server) = tokio::io::duplex(1024);
        let client = start(Box::pin(client), [7; 32], "test");
        assert!(!client.out_tx.belongs_to(&[1; 32]));
        assert!(client
            .out_tx
            .deposit([1; 32], vec![1, 2], None)
            .await
            .is_err());
        client.out_tx.send(ClientToRelay::Ping).await.unwrap();
        assert!(matches!(
            recv(&mut server).await.unwrap(),
            ClientToRelay::Ping
        ));
    }

    #[tokio::test]
    async fn sender_never_accepts_an_early_or_unmatched_deposit_receipt() {
        for failure in [0, 1, 2] {
            let (client, mut server) = tokio::io::duplex(65_536);
            let client = start(Box::pin(client), [4; 32], "test");
            let relay = tokio::spawn(async move {
                for i in 0..2 {
                    assert!(
                        matches!(recv(&mut server).await.unwrap(), ClientToRelay::SendPartV1 {index, total: 2, ..} if index==i)
                    );
                }
                let first = match failure {
                    0 => RelayToClient::Deposited { id: 8 },
                    1 => RelayToClient::SendPartAckV1 { index: 1 },
                    _ => RelayToClient::SendPartAckV1 { index: 0 },
                };
                send(&mut server, &first).await.unwrap();
                if failure == 2 {
                    send(&mut server, &RelayToClient::Deposited { id: 0 })
                        .await
                        .unwrap();
                }
            });
            assert!(client
                .out_tx
                .deposit([4; 32], vec![6, 7], None)
                .await
                .is_err());
            relay.await.unwrap();
        }
    }
}
