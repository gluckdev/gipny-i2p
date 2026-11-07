use anyhow::{Result, ensure};
use clap::Parser;
use gipny_browser_router::bootstrap::is_public_ipv4;
use gipny_forwarder::{Policy, app, load_targets};
use std::{
    collections::HashSet,
    net::{Ipv4Addr, SocketAddrV4},
    path::PathBuf,
    sync::{Arc, PoisonError},
    time::Duration,
};

#[derive(Parser)]
#[command(
    about = "Bounded WSS/WebRTC NTCP2 forwarder; TLS terminates at an operator's reverse proxy"
)]
struct Args {
    #[arg(long, default_value = "127.0.0.1:18767")]
    listen: SocketAddrV4,
    #[arg(long, required = true)]
    host: Vec<String>,
    #[arg(long, required = true)]
    origin: Vec<String>,
    #[arg(long)]
    capability_file: PathBuf,
    #[arg(long)]
    candidates: PathBuf,
    #[arg(long)]
    allow_fixture_loopback: bool,
    #[arg(long)]
    allow_fixture_ice_private: bool,
    #[arg(long, default_value_t = 16)]
    max_connections: usize,
    #[arg(long, default_value_t = 600)]
    session_seconds: u64,
    #[arg(long, default_value_t = 120)]
    idle_seconds: u64,
    #[arg(long, default_value_t = 67108864)]
    max_bytes: usize,
    /// UDP bind for WebRTC. Port 0 = one ephemeral port per session. A
    /// wildcard or private address requires --rtc-public-ip.
    #[arg(long)]
    rtc_bind: Option<SocketAddrV4>,
    /// Public IPv4 advertised in every answer candidate (1:1 NAT / cloud VM).
    #[arg(long)]
    rtc_public_ip: Option<Ipv4Addr>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    // Public plaintext listeners are deliberately unavailable. Terminate TLS
    // at a local proxy with header/body/time/rate limits and no auth logging.
    ensure!(
        args.listen.ip().is_loopback(),
        "HTTP listener must be loopback behind TLS"
    );
    ensure!(
        (1..=600).contains(&args.session_seconds)
            && (1..=300).contains(&args.idle_seconds)
            && args.idle_seconds <= args.session_seconds
            && (1..=67108864).contains(&args.max_bytes),
        "invalid limits"
    );
    ensure!(
        std::fs::metadata(&args.capability_file)?.len() <= 128,
        "invalid capability file"
    );
    let capability = std::fs::read_to_string(&args.capability_file)?;
    let targets = load_targets(&args.candidates, args.allow_fixture_loopback)?;
    let mut policy = Policy::new(
        capability.trim(),
        HashSet::from_iter(args.host),
        HashSet::from_iter(args.origin),
        targets,
        args.max_connections,
    )?;
    policy.session = Duration::from_secs(args.session_seconds);
    policy.idle = Duration::from_secs(args.idle_seconds);
    policy.max_bytes = args.max_bytes;
    // WebRTC answers must not advertise internal interfaces (VPC, docker0):
    // a wildcard/private bind needs --rtc-public-ip. Without any RTC option
    // /rtc is disabled (404); --allow-fixture-ice-private is CI-only.
    if let Some(ip) = args.rtc_public_ip {
        ensure!(
            is_public_ipv4(ip.into()),
            "RTC advertised IP must be public"
        );
    } else if let Some(bind) = args.rtc_bind {
        ensure!(
            is_public_ipv4((*bind.ip()).into()) || args.allow_fixture_ice_private,
            "--rtc-bind on a wildcard or private address requires --rtc-public-ip"
        );
    }
    let rtc_bind = args
        .rtc_bind
        .unwrap_or(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0));
    // webrtc 0.21.1 binds one socket per PeerConnection without SO_REUSEADDR
    // and has no UDP mux: a fixed port admits a single concurrent RTC session.
    ensure!(
        rtc_bind.port() == 0 || args.max_connections == 1,
        "--rtc-bind with a fixed port supports only --max-connections 1; use port 0"
    );
    policy.rtc_bind = rtc_bind;
    policy.rtc_public_ip = args.rtc_public_ip;
    policy.fixture_ice_private = args.allow_fixture_ice_private;
    let listener = tokio::net::TcpListener::bind(args.listen).await?;
    println!(
        "READY targets={} rtc={}",
        policy
            .targets
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .len(),
        if policy.rtc_enabled() { "on" } else { "off" }
    );
    let policy = Arc::new(policy);
    let reload_policy = policy.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(300)).await;
            let path = args.candidates.clone();
            if let Ok(Ok(targets)) = tokio::task::spawn_blocking(move || {
                load_targets(&path, args.allow_fixture_loopback)
            })
            .await
            {
                *reload_policy
                    .targets
                    .write()
                    .unwrap_or_else(PoisonError::into_inner) = targets;
            }
            // Failure never extends old leases; their original deadlines stand.
        }
    });
    // No tracing subscriber: capability headers, SDP and ciphertext are never logged.
    axum::serve(listener, app(policy))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
