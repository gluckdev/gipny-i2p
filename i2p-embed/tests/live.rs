//! Over the live i2p network: a router in this process, two destinations on
//! it, a stream from one to the other and back. Ignored by default (it needs
//! the network and a few minutes); CI runs it with `--ignored`.

use std::time::{Duration, Instant};

use i2p_embed::{Destination, DestinationOptions, Router};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn a_stream_between_two_destinations_in_one_router() {
    let dir = std::env::var("I2P_EMBED_TEST_DIR").unwrap_or_else(|_| {
        std::env::temp_dir().join("i2p-embed-live").to_string_lossy().into_owned()
    });
    std::fs::create_dir_all(&dir).unwrap();
    let t0 = Instant::now();
    let router = Router::start(&[format!("--datadir={dir}"), "--notransit".into(), "--reseed.verify=true".into()], None).expect("router");
    assert!(
        std::path::Path::new(&dir).join("certificates/reseed").read_dir().is_ok_and(|mut d| d.next().is_some()),
        "no reseed certificates laid out"
    );

    let server = Destination::new(&router, Some(&i2p_embed::generate_keys()), &DestinationOptions { publish: true, ..Default::default() }).unwrap();
    let client = Destination::new(&router, None, &DestinationOptions::default()).unwrap();
    assert!(i2p_embed::public_of(&i2p_embed::generate_keys()).is_ok());
    server.ready(Duration::from_secs(600)).await.expect("server ready");
    client.ready(Duration::from_secs(600)).await.expect("client ready");
    eprintln!("both destinations ready in {:?}", t0.elapsed());

    let mut inbound = server.accept();
    let echo = tokio::spawn(async move {
        let mut s = inbound.recv().await.expect("an inbound stream");
        // The server speaks first, as the relay does (its Challenge): the
        // stream must reach it before the client has written anything.
        s.write_all(b"ready").await.unwrap();
        s.flush().await.unwrap();
        let mut buf = [0u8; 5];
        s.read_exact(&mut buf).await.expect("read on the server");
        s.write_all(&buf).await.unwrap();
        s.flush().await.unwrap();
        tokio::time::sleep(Duration::from_secs(5)).await;
    });

    let t1 = Instant::now();
    let mut s = tokio::time::timeout(Duration::from_secs(300), async {
        loop {
            match client.connect(server.address(), 0).await {
                Ok(s) => break s,
                // The LeaseSet may not have reached the floodfills yet.
                Err(e) => { eprintln!("connect: {e}; again"); tokio::time::sleep(Duration::from_secs(5)).await; }
            }
        }
    }).await.expect("connected within 300 s");
    eprintln!("connected in {:?}", t1.elapsed());
    let mut first = [0u8; 5];
    tokio::time::timeout(Duration::from_secs(120), s.read_exact(&mut first)).await
        .expect("the server's first words in 120 s, before the client wrote anything").unwrap();
    assert_eq!(&first, b"ready");
    s.write_all(b"hello").await.unwrap();
    s.flush().await.unwrap();
    let mut back = [0u8; 5];
    tokio::time::timeout(Duration::from_secs(120), s.read_exact(&mut back)).await.expect("echo in 120 s").unwrap();
    assert_eq!(&back, b"hello");
    eprintln!("echo in {:?}", t1.elapsed());
    echo.await.unwrap();
}
