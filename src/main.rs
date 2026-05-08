mod config;
mod model;
mod solana;
mod state;
mod ws;

use std::str::FromStr;

use anyhow::Result;
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderValue, Method, StatusCode},
    routing::get,
};
use serde_json::json;
use tower_http::{cors::{Any, CorsLayer}, trace::TraceLayer};
use tracing::info;
use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

use crate::{
    config::Config,
    model::{HealthzResponse, ReadyzResponse},
    state::AppState,
};

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();

    let config = std::sync::Arc::new(Config::from_env()?);
    let state = AppState::new(config.clone()).await?;
    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/config", get(public_config))
        .route("/ws", get(ws::ws_handler))
        .with_state(state)
        .layer(TraceLayer::new_for_http())
        .layer(cors_layer(&config.cors_origin));

    let listener = tokio::net::TcpListener::bind(config.bind_addr()).await?;
    info!(address = %config.bind_addr(), "shellshock relay listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

async fn healthz() -> Json<HealthzResponse> {
    Json(HealthzResponse { status: "ok" })
}

async fn readyz(State(state): State<AppState>) -> (StatusCode, Json<ReadyzResponse>) {
    let readiness = state.readiness().await;
    let status = if readiness.status == "ok" {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (status, Json(readiness))
}

async fn public_config(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(json!(state.config.public()))
}

fn cors_layer(origin: &str) -> CorsLayer {
    let base = CorsLayer::new()
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
        .allow_headers(Any);

    if origin == "*" {
        base.allow_origin(Any)
    } else {
        match HeaderValue::from_str(origin) {
            Ok(value) => base.allow_origin(value),
            Err(_) => base.allow_origin(Any),
        }
    }
}

fn init_tracing() {
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        let mut signal = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("TERM handler");
        signal.recv().await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
