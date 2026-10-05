use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use gipny_lib::web_server;
use gipny_lib::AppCtx;
use gipny_libcore::security::ServerBind;

fn print_help() {
    eprintln!(
        "gipny-web: Headless Web Gateway for Gipny Messenger\n\n\
        Usage: gipny-web [OPTIONS]\n\n\
        Options:\n\
          --listen <ADDR>     Socket address to bind (default: 0.0.0.0:8080, env: GIPNY_WEB_LISTEN)\n\
          --static <DIR>      Path to static frontend web build (default: ./ui/dist, env: GIPNY_WEB_STATIC)\n\
          --data-dir <DIR>    Base data directory (default: ./data, env: GIPNY_DATA_DIR)\n\
          --hops <COUNT>      Server tunnel hops override (default: 1, env: GIPNY_SERVER_HOPS)\n\
          --help, -h          Print this help message\n"
    );
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut listen_addr: SocketAddr = std::env::var("GIPNY_WEB_LISTEN")
        .unwrap_or_else(|_| "0.0.0.0:8080".to_string())
        .parse()
        .unwrap_or_else(|_| "0.0.0.0:8080".parse().unwrap());

    let mut static_dir = PathBuf::from(
        std::env::var("GIPNY_WEB_STATIC").unwrap_or_else(|_| "./ui/dist".to_string()),
    );

    let mut data_dir = PathBuf::from(
        std::env::var("GIPNY_DATA_DIR").unwrap_or_else(|_| "./data".to_string()),
    );

    let mut hops_val = std::env::var("GIPNY_SERVER_HOPS").unwrap_or_else(|_| "1".to_string());

    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--listen" => {
                if i + 1 < args.len() {
                    listen_addr = args[i + 1].parse()?;
                    i += 1;
                }
            }
            "--static" => {
                if i + 1 < args.len() {
                    static_dir = PathBuf::from(&args[i + 1]);
                    i += 1;
                }
            }
            "--data-dir" => {
                if i + 1 < args.len() {
                    data_dir = PathBuf::from(&args[i + 1]);
                    i += 1;
                }
            }
            "--hops" => {
                if i + 1 < args.len() {
                    hops_val = args[i + 1].clone();
                    i += 1;
                }
            }
            "--help" | "-h" => {
                print_help();
                return Ok(());
            }
            _ => {
                eprintln!("Unknown argument: {}", args[i]);
                print_help();
                std::process::exit(1);
            }
        }
        i += 1;
    }

    // Server-only 1-hop rule enforcement
    std::env::set_var("GIPNY_SERVER_HOPS", &hops_val);

    eprintln!("=== Starting Gipny Web Messenger ===");
    eprintln!("Listen:   {}", listen_addr);
    eprintln!("Static:   {}", static_dir.display());
    eprintln!("Data Dir: {}", data_dir.display());
    eprintln!("Server I2P Hops: {}", hops_val);

    std::fs::create_dir_all(&data_dir)?;

    // Ensure server instance key exists for 3-factor backups
    let server_key_path = data_dir.join("server.key");
    let _ = ServerBind::ensure(&data_dir)?;
    eprintln!("Server bind key: {}", server_key_path.display());

    let ctx = Arc::new(AppCtx::new(data_dir));

    web_server::run_server(ctx, listen_addr, static_dir).await?;

    Ok(())
}
