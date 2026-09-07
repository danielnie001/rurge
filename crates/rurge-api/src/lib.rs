//! Surge-compatible HTTP API (M4 design §5): the phase-1 endpoints, served
//! with axum over the engine's read-only views and the daemon's `Control`.

mod auth;
mod error;
mod routes;

use axum::Router;
use axum::middleware;
use axum::routing::{get, post};
use rurge_config::config::LoadOptions;
use rurge_engine::{Control, Engine};
use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio_util::sync::CancellationToken;

pub use auth::{BAN_DURATION, BAN_FAILURES, BAN_WINDOW};

/// What the API needs from the daemon.
pub struct ApiContext {
    pub engine: Arc<Engine>,
    pub control: Arc<dyn Control>,
    /// The daemon's load options, so `POST /v1/profiles/check` validates the
    /// profile exactly as a reload would.
    pub load_options: LoadOptions,
}

pub(crate) struct Shared {
    pub(crate) engine: Arc<Engine>,
    pub(crate) control: Arc<dyn Control>,
    pub(crate) load_options: LoadOptions,
    pub(crate) auth: auth::AuthState,
    /// Unix seconds when the API came up (`/v1/traffic` `startTime`).
    pub(crate) started_secs: f64,
}

pub(crate) type App = Arc<Shared>;

pub type ServerFuture = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

pub fn router(key: String, ctx: ApiContext) -> Router {
    let app: App = Arc::new(Shared {
        engine: ctx.engine,
        control: ctx.control,
        load_options: ctx.load_options,
        auth: auth::AuthState::new(key),
        started_secs: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0),
    });
    Router::new()
        .route(
            "/v1/outbound",
            get(routes::outbound::get_mode).post(routes::outbound::set_mode),
        )
        .route(
            "/v1/outbound/global",
            get(routes::outbound::get_global).post(routes::outbound::set_global),
        )
        .route(
            "/v1/features/{name}",
            get(routes::features::get_feature).post(routes::features::set_feature),
        )
        .route("/v1/policies", get(routes::policies::policies))
        .route("/v1/rules", get(routes::policies::rules))
        .route("/v1/requests/recent", get(routes::requests::recent))
        .route("/v1/requests/active", get(routes::requests::active))
        .route("/v1/requests/kill", post(routes::requests::kill))
        .route("/v1/traffic", get(routes::traffic::traffic))
        .route("/v1/dns", get(routes::dns::dns))
        .route("/v1/dns/flush", post(routes::dns::flush))
        .route("/v1/test/dns_delay", post(routes::dns::dns_delay))
        .route("/v1/profiles/current", get(routes::profiles::current))
        .route("/v1/profiles/reload", post(routes::profiles::reload))
        .route("/v1/profiles/check", post(routes::profiles::check))
        .route("/v1/log/level", post(routes::log::set_level))
        .route("/v1/modules", get(routes::misc::modules))
        .route("/v1/scripting", get(routes::misc::scripting))
        .route("/v1/events", get(routes::misc::events))
        .route("/v1/stop", post(routes::misc::stop))
        .fallback(routes::misc::not_found)
        .layer(middleware::from_fn_with_state(
            app.clone(),
            auth::require_key,
        ))
        .with_state(app)
}

/// Binds `addr` and returns the bound address plus the server future. The
/// caller spawns the future (the daemon puts it on the engine's task tracker);
/// it completes after `shutdown` is cancelled and in-flight responses finish.
pub async fn serve(
    addr: SocketAddr,
    key: String,
    ctx: ApiContext,
    shutdown: CancellationToken,
) -> io::Result<(SocketAddr, ServerFuture)> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let local = listener.local_addr()?;
    if !local.ip().is_loopback() {
        tracing::warn!(
            %local,
            "http-api listens on a non-loopback address: anyone who reaches it with the key controls this rurge"
        );
    }
    let app = router(key, ctx);
    let fut = async move {
        let result = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async move { shutdown.cancelled().await })
        .await;
        if let Err(e) = result {
            tracing::error!(error = %e, "http-api server stopped with an error");
        }
    };
    Ok((local, Box::pin(fut)))
}
