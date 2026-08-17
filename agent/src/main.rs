mod autostart;
mod balance;
mod ble;
mod ccswitch;
mod cli;
mod codex;
mod codex_oauth;
mod coordinator;
mod http;
mod instance;
mod metrics;
mod producer;
mod protocol;
mod publisher;
mod state;
mod tray;
mod web;

use std::{net::SocketAddr, sync::Arc};

use anyhow::{Context, Result};
use axum::Router;
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

const DEFAULT_PORT: u16 = 38_473;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("epd_agent=info")),
        )
        .init();

    if std::env::args().any(|argument| argument == "--enable-autostart") {
        return autostart::set_enabled(true);
    }
    if std::env::args().any(|argument| argument == "--disable-autostart") {
        return autostart::set_enabled(false);
    }

    let Some(_instance_guard) = instance::acquire()? else {
        tracing::info!("EPD Agent is already running; skipping duplicate startup");
        return Ok(());
    };

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("initialize async runtime")?;

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    return run_desktop(runtime);

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    runtime.block_on(run_headless())
}

async fn prepare_service() -> Result<Service> {
    let state = state::SharedState::new();
    state
        .log(
            "info",
            "agent",
            format!(
                "EPD Agent {} starting on {}",
                env!("EPD_AGENT_VERSION"),
                std::env::consts::OS,
            ),
        )
        .await;
    let ble = ble::BleGateway::spawn(state.clone());
    let (completion_tx, completion_rx) = tokio::sync::mpsc::channel(16);
    let publisher = publisher::ResourcePublisher::spawn(state.clone(), ble.clone(), completion_tx);
    let codex = codex::CodexControl::spawn(producer::ProducerContext {
        state: state.clone(),
        publisher: publisher.clone(),
    });
    let codex_oauth = codex_oauth::CodexOAuthControl::spawn(producer::ProducerContext {
        state: state.clone(),
        publisher: publisher.clone(),
    })?;
    let cli = cli::CliMetricControl::spawn(producer::ProducerContext {
        state: state.clone(),
        publisher: publisher.clone(),
    })?;
    let http = http::HttpMetricControl::spawn(producer::ProducerContext {
        state: state.clone(),
        publisher: publisher.clone(),
    })?;
    let balance = balance::BalanceControl::spawn(producer::ProducerContext {
        state: state.clone(),
        publisher: publisher.clone(),
    })?;
    let ccswitch = ccswitch::CcSwitchControl::spawn(producer::ProducerContext {
        state: state.clone(),
        publisher: publisher.clone(),
    });
    let producers = producer::ProducerRegistry::new(
        &state,
        vec![
            codex.control(),
            codex_oauth.control(),
            ccswitch.control(),
            cli.control(),
            http.control(),
            balance.control(),
        ],
    )
    .await?;
    ccswitch.control().refresh().await?;
    coordinator::SyncCoordinator::spawn(
        state.clone(),
        ble.clone(),
        producers.clone(),
        publisher.clone(),
        completion_rx,
    );
    let context = web::WebContext::new(
        state.clone(),
        ble,
        producers,
        codex_oauth,
        cli,
        http,
        balance,
        publisher,
    )?;
    let port = configured_port()?;
    let launch_url = context.launch_url(port);
    let app = web::router(context);
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = TcpListener::bind(address)
        .await
        .context("bind local management server")?;
    state
        .log(
            "info",
            "web",
            format!("management server listening at http://{address}"),
        )
        .await;
    println!("EPD Agent: {launch_url}");
    Ok(Service {
        state,
        listener,
        app,
        launch_url,
    })
}

struct Service {
    state: Arc<state::SharedState>,
    listener: TcpListener,
    app: Router,
    launch_url: String,
}

async fn open_management_page(state: Arc<state::SharedState>, launch_url: String) {
    match open::that(&launch_url) {
        Ok(()) => state.log("info", "web", "management page opened").await,
        Err(error) => {
            state
                .log(
                    "warn",
                    "web",
                    format!("cannot open management page: {error}"),
                )
                .await
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
async fn run_headless() -> Result<()> {
    let Service {
        state,
        listener,
        app,
        launch_url,
    } = prepare_service().await?;
    if !std::env::args().any(|argument| argument == "--no-open") {
        open_management_page(state, launch_url).await;
    }
    axum::serve(listener, app)
        .await
        .context("serve local management UI")
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn run_desktop(runtime: tokio::runtime::Runtime) -> Result<()> {
    use tao::{
        event::{Event, StartCause},
        event_loop::{ControlFlow, EventLoopBuilder},
    };
    use tray_icon::menu::MenuEvent;

    enum DesktopEvent {
        Menu(MenuEvent),
        ServerStopped,
    }

    let service = runtime.block_on(prepare_service())?;
    let paused = runtime.block_on(service.state.paused());
    let event_loop = EventLoopBuilder::<DesktopEvent>::with_user_event().build();

    #[cfg(target_os = "macos")]
    let mut event_loop = event_loop;

    #[cfg(target_os = "macos")]
    {
        use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
        event_loop.set_activation_policy(ActivationPolicy::Accessory);
        event_loop.set_dock_visibility(false);
        event_loop.set_activate_ignoring_other_apps(false);
    }

    let proxy = event_loop.create_proxy();
    MenuEvent::set_event_handler(Some(move |event| {
        let _ = proxy.send_event(DesktopEvent::Menu(event));
    }));

    let proxy = event_loop.create_proxy();
    let state = service.state.clone();
    runtime.spawn(async move {
        if let Err(error) = axum::serve(service.listener, service.app).await {
            state
                .log(
                    "error",
                    "web",
                    format!("management server stopped: {error}"),
                )
                .await;
        }
        let _ = proxy.send_event(DesktopEvent::ServerStopped);
    });

    let launch_url = service.launch_url;
    let open_on_start = !std::env::args().any(|argument| argument == "--no-open");
    let state = service.state;
    let mut tray = None;
    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::NewEvents(StartCause::Init) => {
                match tray::AgentTray::new(launch_url.clone(), paused) {
                    Ok(icon) => tray = Some(icon),
                    Err(error) => {
                        tracing::error!("cannot create tray icon: {error}");
                        *control_flow = ControlFlow::Exit;
                        return;
                    }
                }
                if open_on_start {
                    runtime.spawn(open_management_page(state.clone(), launch_url.clone()));
                }
            }
            Event::UserEvent(DesktopEvent::Menu(event)) => {
                let Some(action) = tray.as_ref().and_then(|icon| icon.action(&event)) else {
                    return;
                };
                match action {
                    tray::TrayAction::Open(url) => {
                        runtime.spawn(open_management_page(state.clone(), url));
                    }
                    tray::TrayAction::SetPaused(paused) => {
                        let state = state.clone();
                        runtime.spawn(async move { state.set_paused(paused).await });
                    }
                    tray::TrayAction::Quit => *control_flow = ControlFlow::Exit,
                }
            }
            Event::UserEvent(DesktopEvent::ServerStopped) => {
                *control_flow = ControlFlow::Exit;
            }
            _ => {}
        }
    })
}

fn configured_port() -> Result<u16> {
    let arguments = std::env::args().collect::<Vec<_>>();
    if let Some(index) = arguments.iter().position(|argument| argument == "--port") {
        let value = arguments
            .get(index + 1)
            .context("--port requires a value")?;
        return value.parse().context("--port must be a valid TCP port");
    }
    if let Ok(value) = std::env::var("EPD_AGENT_PORT") {
        return value
            .parse()
            .context("EPD_AGENT_PORT must be a valid TCP port");
    }
    Ok(DEFAULT_PORT)
}
