use std::{net::SocketAddr, path::PathBuf, process::{Command, Stdio}, sync::Arc};
use gipny_lib::{data_lock::DataDirLock, resolve_base_dir, web_server::{self, LocalPolicy, WebServerState, PAGES_APP_URL}, AppCtx};
use gipny_libcore::security::{web_launch_token, ServerBind};

fn print_help() {
    eprintln!("gipny-web: experimental local browser UI\n\
Usage: gipny-web [OPTIONS]\n\
  --static <DIR>          Frontend build (default ./ui/dist; GIPNY_WEB_STATIC)\n\
  --data-dir <DIR>        Desktop data directory by default (GIPNY_DATA_DIR)\n\
  --no-open               Print the launch URL without opening a browser\n\
  --pages [<URL>]          Open the Pages app instead of the local UI\n\
  --allow-origin <ORIGIN>  Additional trusted UI origin (repeatable)\n\
  --port <N>              Pin the local loopback port (default 0: automatic)\n\
  --listen <ADDR>         Loopback: local mode; non-loopback: legacy HOSTED GATEWAY\n\
  --hops <COUNT>          Legacy gateway tunnel hops (default 1)\n\
  --help, -h              Show help");
}

fn open_browser(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "windows")]
    let mut command = { use std::os::windows::process::CommandExt;
        let mut c = Command::new("cmd");
        // cmd needs its own quoting: the Pages fragment contains an ampersand.
        // Expanding one variable preserves percent-encoded URLs without a
        // second percent expansion; /v:off also keeps exclamation marks literal.
        c.env("GIPNY_WEB_LAUNCH_URL", url);
        c.raw_arg("/d /v:off /c start \"\" \"%GIPNY_WEB_LAUNCH_URL%\""); c };
    #[cfg(target_os = "macos")]
    let mut command = { let mut c = Command::new("open"); c.arg(url); c };
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let mut command = { let mut c = Command::new("xdg-open"); c.arg(url); c };
    let mut child = command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()?;
    // Reap the launcher without blocking the listener if a fallback browser waits.
    std::thread::spawn(move || { let _ = child.wait(); });
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    gipny_libcore::security::harden_process()?;
    let mut listen: Option<SocketAddr> = None;
    let mut pinned_port: Option<u16> = None;
    let mut static_dir = PathBuf::from(std::env::var("GIPNY_WEB_STATIC").unwrap_or_else(|_| "./ui/dist".into()));
    let mut data_dir = std::env::var_os("GIPNY_DATA_DIR").map(PathBuf::from);
    let mut hops = std::env::var("GIPNY_SERVER_HOPS").unwrap_or_else(|_| "1".into());
    let mut no_open = false;
    let mut pages = None;
    let mut allowed_origins = Vec::new();
    let mut args = std::env::args().skip(1).peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--listen" => listen = Some(args.next().ok_or("--listen needs an address")?.parse()?),
            "--port" => pinned_port = Some(args.next().ok_or("--port needs a number")?.parse()?),
            "--static" => static_dir = args.next().ok_or("--static needs a directory")?.into(),
            "--data-dir" => data_dir = Some(args.next().ok_or("--data-dir needs a directory")?.into()),
            "--hops" => hops = args.next().ok_or("--hops needs a count")?,
            "--allow-origin" => allowed_origins.push(web_server::normalize_origin(&args.next().ok_or("--allow-origin needs an origin")?)?),
            "--no-open" => no_open = true,
            "--pages" => {
                let url = if args.peek().is_some_and(|s| !s.starts_with('-')) { args.next().unwrap() } else { PAGES_APP_URL.into() };
                // A custom launcher is executable UI code and must be explicitly trusted.
                let uri: axum::http::Uri = url.parse()?;
                if !matches!(uri.scheme_str(), Some("http" | "https")) || uri.authority().is_none() || url.contains(['#', '"', '\r', '\n']) {
                    return Err("--pages needs an http(s) URL without a fragment".into());
                }
                pages = Some(url);
            }
            "--help" | "-h" => { print_help(); return Ok(()); }
            _ => return Err(format!("unknown argument: {arg}").into()),
        }
    }
    let listen = web_server::resolve_listen_addr(listen, pinned_port)?;
    let local = listen.ip().is_loopback();
    if !local && pages.is_some() { return Err("--pages requires a loopback listener".into()); }
    if local {
        std::env::remove_var("GIPNY_SERVER_HOPS");
    } else {
        eprintln!("WARNING: non-loopback --listen selects a HOSTED GATEWAY, not local web mode. Keys and data live on this host; token/Origin/Host protections are disabled. Do not expose it to untrusted clients.");
        std::env::set_var("GIPNY_SERVER_HOPS", hops);
    }
    let data_dir = data_dir.unwrap_or_else(|| if local { resolve_base_dir() } else { PathBuf::from("./data") });
    let _data_lock = DataDirLock::acquire(&data_dir)?;
    if !local { let _ = ServerBind::ensure(&data_dir)?; }
    let listener = tokio::net::TcpListener::bind(listen).await?;
    let port = listener.local_addr()?.port();
    let policy = if local {
        let token = web_launch_token();
        let url = match pages {
            Some(url) => format!("{url}#port={port}&token={token}"),
            None => format!("http://127.0.0.1:{port}/#token={token}"),
        };
        eprintln!("Local gipny UI (capability; keep private): {url}");
        if !no_open { if let Err(error) = open_browser(&url) { eprintln!("Could not open browser: {error}. Open the printed URL manually."); } }
        Some(LocalPolicy { port, token, allowed_origins })
    } else { None };
    let ctx = Arc::new(AppCtx::new(data_dir));
    let (event_tx, _) = tokio::sync::broadcast::channel(1024);
    let router = web_server::build_router(Arc::new(WebServerState { ctx, event_tx }), static_dir, policy);
    web_server::serve(listener, router).await?;
    Ok(())
}
