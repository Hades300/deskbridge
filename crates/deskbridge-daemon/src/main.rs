mod build_info;
mod capture;
mod client;
mod debugctl;
mod diag;
mod input;
mod perf;
mod permissions;
mod server;

use crate::input::InputSink;
use anyhow::Result;
use clap::{Parser, Subcommand};
use deskbridge_core::{
    DEFAULT_HEARTBEAT_MS, DebugCommand, DeskBridgeConfig, Edge, InputEvent, InputPacket,
    InputRouter, Layout, Link, Screen, Size, simulate_route,
};
use std::{net::SocketAddr, path::PathBuf, str::FromStr, time::Duration};
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
        #[arg(long, default_value_t = false)]
        reverse_scroll: bool,
        #[arg(long, default_value_t = true)]
        reconnect: bool,
        #[arg(long, default_value_t = false)]
        once: bool,
        #[arg(long)]
        max_events: Option<u64>,
        #[arg(long)]
        stale_after_ms: Option<u64>,
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
        #[arg(long, default_value_t = false)]
        debug_capture_log: bool,
        #[arg(long, default_value_t = false)]
        reverse_scroll: bool,
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
    /// Send a debug command through the server to a connected client.
    Debug {
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(long)]
        server: Option<SocketAddr>,
        #[arg(long, default_value = "mac")]
        name: String,
        #[command(subcommand)]
        command: DebugCliCommand,
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
        #[arg(long, allow_hyphen_values = true)]
        return_dx: Option<i32>,
        #[arg(long, allow_hyphen_values = true)]
        return_dy: Option<i32>,
        #[arg(long, default_value_t = 3)]
        return_steps: usize,
    },
    /// Check platform permissions required by the local DeskBridge process.
    Permissions {
        #[arg(long, default_value_t = false)]
        prompt: bool,
    },
    /// Print the local DeskBridge build and protocol version.
    Version,
    /// Print the display size and mouse location seen by DeskBridge.
    DisplayInfo,
    /// Move the local pointer through the same injection path used by the client.
    InjectTest {
        #[arg(long)]
        x: Option<i32>,
        #[arg(long)]
        y: Option<i32>,
        #[arg(long, default_value_t = 0, allow_hyphen_values = true)]
        dx: i32,
        #[arg(long, default_value_t = 0, allow_hyphen_values = true)]
        dy: i32,
        #[arg(long, default_value_t = false)]
        evented_rel: bool,
    },
    /// Create a default JSON config file.
    InitConfig {
        #[arg(long, default_value = "deskbridge.json")]
        path: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
enum DebugCliCommand {
    /// Read the target client's display size and current mouse location.
    DisplayInfo,
    /// Read the target client's build, platform, and runtime metadata.
    PeerInfo,
    /// Read recent target-side debug log lines kept by the client session.
    Logs,
    /// Read recent server-side diagnostic and connection log lines.
    ServerLogs,
    /// Ask the target client to move its local pointer.
    MoveMouse {
        #[arg(long)]
        x: Option<i32>,
        #[arg(long)]
        y: Option<i32>,
        #[arg(long, default_value_t = 0, allow_hyphen_values = true)]
        dx: i32,
        #[arg(long, default_value_t = 0, allow_hyphen_values = true)]
        dy: i32,
    },
    /// Ask the server to synthesize an edge crossing to the target client and wait for input acks.
    RouteProbe {
        #[arg(long, value_parser = parse_edge)]
        edge: Option<Edge>,
        #[arg(long, default_value_t = 3)]
        steps: u32,
        #[arg(long, default_value_t = 80, allow_hyphen_values = true)]
        dx: i32,
        #[arg(long, default_value_t = 0, allow_hyphen_values = true)]
        dy: i32,
    },
    /// Read the server-side route layout currently used by the active target session.
    RouteStatus,
    /// Read low-latency performance counters for the active route session.
    Perf,
    /// Read or update server-side input settings without restarting the server.
    InputSettings {
        #[arg(long, value_parser = clap::value_parser!(bool))]
        reverse_scroll: Option<bool>,
    },
    /// Inject synthetic capture events into the server capture path and wait for client acks.
    CaptureProbe {
        #[arg(long, value_parser = parse_edge)]
        edge: Option<Edge>,
        #[arg(long, default_value_t = 3)]
        steps: u32,
        #[arg(long, default_value_t = 80, allow_hyphen_values = true)]
        dx: i32,
        #[arg(long, default_value_t = 0, allow_hyphen_values = true)]
        dy: i32,
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
            reverse_scroll,
            reconnect,
            once,
            max_events,
            stale_after_ms,
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
            let stale_after_ms = stale_after_ms
                .or_else(|| config.as_ref().map(|cfg| cfg.reliability.stale_after_ms))
                .unwrap_or(6_000);
            let reverse_scroll =
                reverse_scroll || config.as_ref().is_some_and(|cfg| cfg.input.reverse_scroll);
            client::run(client::ClientOptions {
                server,
                name,
                dry_run,
                reconnect: reconnect && !once,
                reverse_scroll,
                reconnect_max_ms,
                stale_after_ms,
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
            debug_capture_log,
            reverse_scroll,
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
            let reverse_scroll =
                reverse_scroll || config.as_ref().is_some_and(|cfg| cfg.input.reverse_scroll);
            server::run(server::ServerOptions {
                listen,
                name,
                allow,
                demo_events,
                capture,
                debug_capture_log,
                reverse_scroll,
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
        Command::Debug {
            config,
            server,
            name,
            command,
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
            debugctl::run(server, name, debug_cli_command(command)).await
        }
        Command::SimulateRoute {
            config,
            from_screen,
            to_screen,
            edge,
            steps,
            dx,
            dy,
            return_dx,
            return_dy,
            return_steps,
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

            println!("DeskBridge route simulation");
            println!("layout: {from_screen} {edge:?} -> {to_screen}");
            if return_dx.is_some() || return_dy.is_some() {
                simulate_route_with_return(ReturnSimulation {
                    layout: &layout,
                    from_screen: &from_screen,
                    to_screen: &to_screen,
                    edge,
                    outbound_steps: steps,
                    outbound_dx: dx,
                    outbound_dy: dy,
                    return_dx: return_dx.unwrap_or(0),
                    return_dy: return_dy.unwrap_or(0),
                    return_steps,
                })?;
            } else {
                let events =
                    simulate_route(&layout, &from_screen, &to_screen, edge, steps, dx, dy)?;
                for event in events {
                    println!(
                        "event {}: target={} {}",
                        event.index,
                        event.target_screen,
                        describe_input_event(&event.event)
                    );
                }
            }
            println!("result: ok");
            Ok(())
        }
        Command::Permissions { prompt } => permissions::run(prompt),
        Command::Version => {
            println!("DeskBridge");
            for line in build_info::lines() {
                println!("{line}");
            }
            Ok(())
        }
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
        Command::InjectTest {
            x,
            y,
            dx,
            dy,
            evented_rel,
        } => {
            run_inject_test(x, y, dx, dy, evented_rel).await?;
            Ok(())
        }
        Command::InitConfig { path } => {
            DeskBridgeConfig::default().save(&path)?;
            println!("wrote {}", path.display());
            Ok(())
        }
    }
}

fn debug_cli_command(command: DebugCliCommand) -> DebugCommand {
    match command {
        DebugCliCommand::DisplayInfo => DebugCommand::DisplayInfo,
        DebugCliCommand::PeerInfo => DebugCommand::PeerInfo,
        DebugCliCommand::Logs => DebugCommand::RecentLogs,
        DebugCliCommand::ServerLogs => DebugCommand::ServerLogs,
        DebugCliCommand::MoveMouse { x, y, dx, dy } => DebugCommand::MoveMouse { x, y, dx, dy },
        DebugCliCommand::RouteProbe {
            edge,
            steps,
            dx,
            dy,
        } => DebugCommand::RouteProbe {
            edge,
            steps,
            dx,
            dy,
        },
        DebugCliCommand::RouteStatus => DebugCommand::RouteStatus,
        DebugCliCommand::Perf => DebugCommand::Perf,
        DebugCliCommand::InputSettings { reverse_scroll } => {
            DebugCommand::InputSettings { reverse_scroll }
        }
        DebugCliCommand::CaptureProbe {
            edge,
            steps,
            dx,
            dy,
        } => DebugCommand::CaptureProbe {
            edge,
            steps,
            dx,
            dy,
        },
    }
}

fn load_config(path: Option<PathBuf>) -> Result<Option<DeskBridgeConfig>> {
    path.map(DeskBridgeConfig::load)
        .transpose()
        .map_err(Into::into)
}

struct ReturnSimulation<'a> {
    layout: &'a Layout,
    from_screen: &'a str,
    to_screen: &'a str,
    edge: Edge,
    outbound_steps: usize,
    outbound_dx: i32,
    outbound_dy: i32,
    return_dx: i32,
    return_dy: i32,
    return_steps: usize,
}

fn simulate_route_with_return(options: ReturnSimulation<'_>) -> Result<()> {
    let (x, y) = simulation_edge_point(options.layout, options.from_screen, options.edge)?;
    let mut router = InputRouter::new(options.layout.clone(), options.from_screen.to_string())?;
    let first = router.observe_local_pointer(x, y).ok_or_else(|| {
        anyhow::anyhow!(
            "no linked transition from {} on {:?}",
            options.from_screen,
            options.edge
        )
    })?;
    if first.target_screen != options.to_screen {
        anyhow::bail!(
            "transition targeted '{}', expected '{}'",
            first.target_screen,
            options.to_screen
        );
    }

    println!(
        "event 0: target={} {}",
        first.target_screen,
        describe_input_event(&first.event)
    );

    let mut index = 0;
    for _ in 0..options.outbound_steps {
        index += 1;
        let routed = router
            .route_if_remote_active(InputEvent::MouseMove {
                dx: options.outbound_dx,
                dy: options.outbound_dy,
            })
            .ok_or_else(|| anyhow::anyhow!("remote screen stopped receiving routed input"))?;
        println!(
            "event {index}: target={} {}",
            routed.target_screen,
            describe_input_event(&routed.event)
        );
    }

    for _ in 0..options.return_steps {
        index += 1;
        match router.route_if_remote_active(InputEvent::MouseMove {
            dx: options.return_dx,
            dy: options.return_dy,
        }) {
            Some(routed) => println!(
                "event {index}: target={} {}",
                routed.target_screen,
                describe_input_event(&routed.event)
            ),
            None => {
                println!("release {index}: active={}", router.active_screen());
                return Ok(());
            }
        }
    }

    println!("release: not reached; active={}", router.active_screen());
    Ok(())
}

fn simulation_edge_point(layout: &Layout, screen_name: &str, edge: Edge) -> Result<(u32, u32)> {
    let screen = layout
        .screens
        .iter()
        .find(|screen| screen.name == screen_name)
        .ok_or_else(|| anyhow::anyhow!("layout does not include screen '{screen_name}'"))?;
    let max_x = screen.size.width.saturating_sub(1);
    let max_y = screen.size.height.saturating_sub(1);
    let mid_x = screen.size.width / 2;
    let mid_y = screen.size.height / 2;

    Ok(match edge {
        Edge::Left => (0, mid_y),
        Edge::Right => (max_x, mid_y),
        Edge::Top => (mid_x, 0),
        Edge::Bottom => (mid_x, max_y),
    })
}

async fn run_inject_test(
    x: Option<i32>,
    y: Option<i32>,
    dx: i32,
    dy: i32,
    evented_rel: bool,
) -> Result<()> {
    let before = input::display_info()?;
    let mut sink = input::EnigoSink::new()?;

    println!("DeskBridge injection test");
    println!(
        "before: display={}x{} location={}",
        before.size.width,
        before.size.height,
        format_location(before.location)
    );

    match (x, y) {
        (Some(x), Some(y)) => {
            sink.apply(&InputPacket {
                seq: 1,
                event: InputEvent::MouseAbs { x, y },
            })
            .await?;
        }
        (None, None) => {}
        _ => anyhow::bail!("--x and --y must be provided together"),
    }

    if dx != 0 || dy != 0 {
        if evented_rel {
            sink.move_mouse_rel_evented_for_diagnostics(dx, dy)?;
        } else {
            sink.apply(&InputPacket {
                seq: 2,
                event: InputEvent::MouseMove { dx, dy },
            })
            .await?;
        }
    }

    tokio::time::sleep(Duration::from_millis(100)).await;
    let after = input::display_info()?;
    println!(
        "after: display={}x{} location={}",
        after.size.width,
        after.size.height,
        format_location(after.location)
    );

    Ok(())
}

fn format_location(location: Option<(i32, i32)>) -> String {
    location
        .map(|(x, y)| format!("x={x} y={y}"))
        .unwrap_or_else(|| "unavailable".to_string())
}

fn default_layout(server_name: &str, clients: &[String]) -> Layout {
    let mut screens = vec![Screen {
        name: server_name.to_string(),
        size: Size {
            width: 1920,
            height: 1080,
        },
        origin: None,
    }];

    for client in clients {
        screens.push(Screen {
            name: client.clone(),
            size: Size {
                width: 1728,
                height: 1117,
            },
            origin: None,
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
