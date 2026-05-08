use std::{env, net::SocketAddr, str::FromStr};

use anyhow::{Context, Result, anyhow};
use serde::Serialize;
use solana_sdk::pubkey::Pubkey;

#[derive(Clone, Debug)]
pub struct Config {
    pub port: u16,
    pub solana_rpc_http_url: String,
    pub solana_rpc_ws_url: String,
    pub program_id: String,
    pub cors_origin: String,
    pub turn_timeout_seconds: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct PublicConfig {
    pub program_id: String,
    pub solana_rpc_http_url: String,
    pub solana_rpc_ws_url: String,
    pub turn_timeout_seconds: u64,
    pub websocket_path: &'static str,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let port = env::var("PORT")
            .unwrap_or_else(|_| "8080".to_string())
            .parse::<u16>()
            .context("invalid PORT")?;
        let solana_rpc_http_url = env::var("SOLANA_RPC_HTTP_URL")
            .unwrap_or_else(|_| "https://api.devnet.solana.com".to_string());
        let solana_rpc_ws_url = env::var("SOLANA_RPC_WS_URL")
            .unwrap_or_else(|_| "wss://api.devnet.solana.com".to_string());
        let program_id = env::var("PROGRAM_ID")
            .unwrap_or_else(|_| "11111111111111111111111111111111".to_string());
        let cors_origin =
            env::var("CORS_ORIGIN").unwrap_or_else(|_| "http://localhost:5173".to_string());
        let turn_timeout_seconds = env::var("TURN_TIMEOUT_SECONDS")
            .unwrap_or_else(|_| "90".to_string())
            .parse::<u64>()
            .context("invalid TURN_TIMEOUT_SECONDS")?;

        Ok(Self {
            port,
            solana_rpc_http_url,
            solana_rpc_ws_url,
            program_id,
            cors_origin,
            turn_timeout_seconds,
        })
    }

    pub fn bind_addr(&self) -> SocketAddr {
        SocketAddr::from(([0, 0, 0, 0], self.port))
    }

    pub fn public(&self) -> PublicConfig {
        PublicConfig {
            program_id: self.program_id.clone(),
            solana_rpc_http_url: self.solana_rpc_http_url.clone(),
            solana_rpc_ws_url: self.solana_rpc_ws_url.clone(),
            turn_timeout_seconds: self.turn_timeout_seconds,
            websocket_path: "/ws",
        }
    }

    pub fn validate_program_id(&self) -> Result<Pubkey> {
        Pubkey::from_str(&self.program_id)
            .map_err(|error| anyhow!("PROGRAM_ID is not a valid pubkey: {error}"))
    }
}
