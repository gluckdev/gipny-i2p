use crate::BrowserTcpStream;
use emissary_core::runtime::{TcpListener, UdpSocket};
use std::{
    net::SocketAddr,
    pin::Pin,
    task::{Context, Poll},
};

/// Browsers cannot listen on TCP or bind UDP. Explicit failure is preferable to
/// a pending future that would leave router initialization stuck indefinitely.
pub struct BrowserTcpListener;

impl TcpListener<BrowserTcpStream> for BrowserTcpListener {
    async fn bind(_: SocketAddr) -> Option<Self> {
        None
    }
    fn poll_accept(&mut self, _: &mut Context<'_>) -> Poll<Option<(BrowserTcpStream, SocketAddr)>> {
        Poll::Ready(None)
    }
    fn local_address(&self) -> Option<SocketAddr> {
        None
    }
}

#[derive(Clone)]
pub struct BrowserUdpSocket;

impl UdpSocket for BrowserUdpSocket {
    async fn bind(_: SocketAddr) -> Option<Self> {
        None
    }
    async fn bind_with_mtu(_: SocketAddr, _: usize) -> Option<Self> {
        None
    }
    async fn send_to(&mut self, _: &[u8], _: SocketAddr) -> Option<usize> {
        None
    }
    async fn recv_from(&mut self, _: &mut [u8]) -> Option<(usize, SocketAddr)> {
        None
    }
    fn poll_send_to(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        _: &[u8],
        _: SocketAddr,
    ) -> Poll<Option<usize>> {
        Poll::Ready(None)
    }
    fn poll_recv_from(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        _: &mut [u8],
    ) -> Poll<Option<(usize, SocketAddr)>> {
        Poll::Ready(None)
    }
    fn local_address(&self) -> Option<SocketAddr> {
        None
    }
    fn mtu(&self) -> usize {
        0
    }
}
