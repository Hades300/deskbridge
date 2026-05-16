mod capture;
mod client;
mod diag;
mod input;
mod permissions;
mod server;

use anyhow::Result;
use clap::{Parser, Subcommand};
use deskbridge_core::{DEFAULT_HEARTBEAT_MS, DeskBridgeConfig, Edge, Layout, Link, Screen, Size};
use std::{net::SocketAddr, path::PathBuf};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(name = "deskbridge")]
#[command(about = "A native-feel, self-healing keyboard and mouse bridge")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run a client that receives remote input events.
    Client {
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(long, env = "DESKBRIDGE_SERVER")]
        server: Option<SocketAddr>,
        #[arg(long, default_value = "mac", env = "DESKBRIDGE_NAME")]
        name: String,
        #[arg(long, default_value_t = false)]
        dry_run: bool,
        #[arg(long, default_value_t = true)]
        reconnect: bool,
        #[arg(long, default_value_t = false)]
        once: bool,
        #[arg(long)]
        max_events: Option<u64>,
    },
    /// Run a server that accepts clients and can emit demo input events.
    Server {
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(long, default_value = "0.0.0.0:24800")]
        listen: SocketAddr,
        #[arg(long, default_value = "windows")]
        name: String,
        #[arg(long, value_delimiter = ',', default_value = "mac")]
        allow: Vec<String>,
        #[arg(long, default_value_t = false)]
        demo_events: bool,
        #[arg(long, default_value_t = false)]
        capture: bool,
    },
    /// Diagnose reachability and protocol handshake.
    Diag {
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(long)]
        server: Option<SocketAddr>,
        #[arg(long, default_value = "mac")]
        name: String,
    },
    /// Check platform permissions required by the local DeskBridge process.
    Permissions {
        #[arg(long, default_value_t = false)]
        prompt: bool,
    },
    /// Create a default JSON config file.
    InitConfig {
        #[arg(long, default_value = "deskbridge.json")]
        path: PathBuf,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    let cli = Cli::parse();

    match cli.command {
        Command::Client {
            config,
            server,
            name,
            dry_run,
            reconnect,
            once,
            max_events,
        } => {
            let config = load_config(config)?;
            let server = server
                .or_else(|| {
                    config
                        .as_ref()
                        .and_then(|cfg| cfg.client.server_addr.parse().ok())
                })
                .ok_or_else(|| anyhow::anyhow!("--server is required when --config is not used"))?;
            let name = config
                .as_ref()
                .map(|cfg| cfg.client.name.clone())
                .unwrap_or(name);
            let reconnect_max_ms = config
                .as_ref()
                .map(|cfg| cfg.reliability.reconnect_max_ms)
                .unwrap_or(10_000);
            client::run(client::ClientOptions {
                server,
                name,
                dry_run,
                reconnect: reconnect && !once,
                reconnect_max_ms,
                max_events,
            })
            .await
        }
        Command::Server {
            config,
            listen,
            name,
            allow,
            demo_events,
            capture,
        } => {
            let config = load_config(config)?;
            let listen = config
                .as_ref()
                .and_then(|cfg| cfg.server.listen.parse().ok())
                .unwrap_or(listen);
            let name = config
                .as_ref()
                .map(|cfg| cfg.server.name.clone())
                .unwrap_or(name);
            let allow = config
                .as_ref()
                .map(|cfg| cfg.layout.allowed_clients(&cfg.server.name))
                .unwrap_or(allow);
            let heartbeat_ms = config
                .as_ref()
                .map(|cfg| cfg.reliability.heartbeat_ms)
                .unwrap_or(DEFAULT_HEARTBEAT_MS);
            let layout = config
                .as_ref()
                .map(|cfg| cfg.layout.clone())
                .unwrap_or_else(|| default_layout(&name, &allow));
            server::run(server::ServerOptions {
                listen,
                name,
                allow,
                demo_events,
                capture,
                heartbeat_ms,
                layout,
            })
            .await
        }
        Command::Diag {
            config,
            server,
            name,
        } => {
            let config = load_config(config)?;
            let server = server
                .or_else(|| {
                    config
                        .as_ref()
                        .and_then(|cfg| cfg.client.server_addr.parse().ok())
                })
                .ok_or_else(|| anyhow::anyhow!("--server is required when --config is not used"))?;
            let name = config
                .as_ref()
                .map(|cfg| cfg.client.name.clone())
                .unwrap_or(name);
            diag::run(server, name).await
        }
        Command::Permissions { prompt } => permissions::run(prompt),
        Command::InitConfig { path } => {
            DeskBridgeConfig::default().save(&path)?;
            println!("wrote {}", path.display());
            Ok(())
        }
    }
}

fn load_config(path: Option<PathBuf>) -> Result<Option<DeskBridgeConfig>> {
    path.map(DeskBridgeConfig::load)
        .transpose()
        .map_err(Into::into)
}

fn default_layout(server_name: &str, clients: &[String]) -> Layout {
    let mut screens = vec![Screen {
        name: server_name.to_string(),
        size: Size {
            width: 1920,
            height: 1080,
        },
    }];

    for client in clients {
        screens.push(Screen {
            name: client.clone(),
            size: Size {
                width: 1728,
                height: 1117,
            },
        });
    }

    let links = clients
        .first()
        .map(|client| {
            vec![Link {
                from: server_name.to_string(),
                edge: Edge::Right,
                to: client.clone(),
            }]
        })
        .unwrap_or_default();

    Layout { screens, links }
}

fn init_tracing() {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("deskbridge=info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();
}
