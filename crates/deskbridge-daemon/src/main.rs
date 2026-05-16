mod capture;
mod client;
mod diag;
mod input;
mod permissions;
mod server;

use anyhow::Result;
use clap::{Parser, Subcommand};
use deskbridge_core::{
    DEFAULT_HEARTBEAT_MS, DeskBridgeConfig, Edge, InputEvent, Layout, Link, Screen, Size,
    simulate_route,
};
use std::{net::SocketAddr, path::PathBuf, str::FromStr};
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
    /// Simulate a configured edge crossing and continued remote mouse movement.
    SimulateRoute {
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(long = "from")]
        from_screen: Option<String>,
        #[arg(long = "to")]
        to_screen: Option<String>,
        #[arg(long, default_value = "right", value_parser = parse_edge)]
        edge: Edge,
        #[arg(long, default_value_t = 5)]
        steps: usize,
        #[arg(long, default_value_t = 120, allow_hyphen_values = true)]
        dx: i32,
        #[arg(long, default_value_t = 0, allow_hyphen_values = true)]
        dy: i32,
    },
    /// Check platform permissions required by the local DeskBridge process.
    Permissions {
        #[arg(long, default_value_t = false)]
        prompt: bool,
    },
    /// Print the display size and mouse location seen by DeskBridge.
    DisplayInfo,
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
        Command::SimulateRoute {
            config,
            from_screen,
            to_screen,
            edge,
            steps,
            dx,
            dy,
        } => {
            let config = load_config(config)?;
            let from_screen = from_screen
                .or_else(|| config.as_ref().map(|cfg| cfg.server.name.clone()))
                .unwrap_or_else(|| "windows".to_string());
            let to_screen = to_screen
                .or_else(|| config.as_ref().map(|cfg| cfg.client.name.clone()))
                .unwrap_or_else(|| "mac".to_string());
            let layout = config
                .as_ref()
                .map(|cfg| cfg.layout.clone())
                .unwrap_or_else(|| default_layout(&from_screen, std::slice::from_ref(&to_screen)));

            let events = simulate_route(&layout, &from_screen, &to_screen, edge, steps, dx, dy)?;
            println!("DeskBridge route simulation");
            println!("layout: {from_screen} {edge:?} -> {to_screen}");
            for event in events {
                println!(
                    "event {}: target={} {}",
                    event.index,
                    event.target_screen,
                    describe_input_event(&event.event)
                );
            }
            println!("result: ok");
            Ok(())
        }
        Command::Permissions { prompt } => permissions::run(prompt),
        Command::DisplayInfo => {
            let info = input::display_info()?;
            println!("DeskBridge display info");
            println!("main_display: {}x{}", info.size.width, info.size.height);
            if let Some((x, y)) = info.location {
                println!("mouse_location: x={x} y={y}");
            } else {
                println!("mouse_location: unavailable");
            }
            Ok(())
        }
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

fn parse_edge(value: &str) -> Result<Edge, String> {
    Edge::from_str(value)
}

fn describe_input_event(event: &InputEvent) -> String {
    match event {
        InputEvent::MouseMove { dx, dy } => format!("MouseMove dx={dx} dy={dy}"),
        InputEvent::MouseAbs { x, y } => format!("MouseAbs x={x} y={y}"),
        InputEvent::MouseButton { button, state } => {
            format!("MouseButton button={button:?} state={state:?}")
        }
        InputEvent::Wheel { dx, dy } => format!("Wheel dx={dx} dy={dy}"),
        InputEvent::Key { key, state } => format!("Key key={key} state={state:?}"),
        InputEvent::Text { text } => format!("Text text={text:?}"),
    }
}
