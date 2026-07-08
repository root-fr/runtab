mod api;
mod api_savings;
mod api_tools;
mod background;
pub(crate) mod browser;
mod dto;
#[cfg(feature = "embed-ui")]
mod embed;
mod router;

use std::io::ErrorKind;
use std::sync::{Arc, Mutex};

use tokio::net::TcpListener;

use crate::ledger::Ledger;
use crate::pricing::Pricing;

use api::AppState;

const DEFAULT_PORT: u16 = 7822;
const MAX_PORT_TRIES: u16 = 32;

/// Ports the 782x family reserves for non-dashboard services (sync server, Mailpit
/// UI, Mailpit SMTP). The busy-port walk skips them so a second dashboard never
/// auto-lands on the sync server's port — which would turn every local push into
/// a 405 against the dashboard and a permanent "degraded" state (spec port table).
const RESERVED_PORTS: [u16; 3] = [7824, 7825, 7826];

/// Run the local dashboard: bind `127.0.0.1` (auto-increment if busy), open the
/// browser, and serve the SPA + `/api` immediately. The initial scan is just the
/// background loop's first tick — a large backfill must never keep the port
/// unbound and the terminal silent.
pub fn run(ledger: Ledger, pricing: Pricing, port: Option<u16>) -> anyhow::Result<()> {
    let state = AppState {
        ledger: Arc::new(Mutex::new(ledger)),
    };
    let pricing = Arc::new(pricing);
    let start_port = resolve_port(port);

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;

    runtime.block_on(async move {
        let (listener, bound) = bind(start_port).await?;
        let url = format!("http://127.0.0.1:{bound}");
        println!("runtab dashboard on {url}");
        println!("runtab: scanning agent logs in the background; charts fill in as data lands");
        browser::open(&url);

        tokio::spawn(background::run_loop(state.clone(), pricing));

        axum::serve(listener, router::router(state).into_make_service())
            .with_graceful_shutdown(shutdown_signal())
            .await
            .map_err(anyhow::Error::from)
    })
}

/// Build the dashboard router over an already-open ledger. Exposed so the HTTP
/// surface can be exercised without binding a socket.
pub fn app(ledger: Arc<Mutex<Ledger>>) -> axum::Router {
    router::router(AppState { ledger })
}

pub(crate) fn resolve_port(port: Option<u16>) -> u16 {
    port.or_else(|| {
        std::env::var("RUNTAB_PORT")
            .ok()
            .and_then(|s| s.trim().parse().ok())
    })
    .unwrap_or(DEFAULT_PORT)
}

/// Bind interface: loopback by default so the dashboard stays private, with a
/// `RUNTAB_BIND_ADDR` override (e.g. `0.0.0.0` for the docker validation stack)
/// that mirrors the sync server's `RUNTAB_BIND_ADDR`.
fn bind_host() -> String {
    std::env::var("RUNTAB_BIND_ADDR")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "127.0.0.1".to_string())
}

/// Bind the first free port at or above `start`, so a second `runtab serve`
/// never fails on a busy port — it just moves up (spec §Local dashboard).
async fn bind(start: u16) -> anyhow::Result<(TcpListener, u16)> {
    let host = bind_host();
    let mut last_err = None;
    for offset in 0..MAX_PORT_TRIES {
        let port = start.saturating_add(offset);
        // Honour an explicit start port even if reserved, but never auto-increment
        // onto another service's port.
        if offset > 0 && RESERVED_PORTS.contains(&port) {
            continue;
        }
        match TcpListener::bind((host.as_str(), port)).await {
            Ok(l) => return Ok((l, port)),
            Err(e) if e.kind() == ErrorKind::AddrInUse => last_err = Some(e),
            Err(e) => return Err(e.into()),
        }
    }
    Err(anyhow::anyhow!(
        "no free port in {start}..{}: {}",
        start.saturating_add(MAX_PORT_TRIES),
        last_err
            .map(|e| e.to_string())
            .unwrap_or_else(|| "address in use".to_string())
    ))
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
