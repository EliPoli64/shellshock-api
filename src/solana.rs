use std::{str::FromStr, time::Duration};

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use solana_sdk::pubkey::Pubkey;
use tokio::time::sleep;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{error, info, warn};

use crate::state::AppState;

pub async fn spawn_room_observer(state: AppState, room_pubkey: String) {
    tokio::spawn(async move {
        let parsed_room = match Pubkey::from_str(&room_pubkey) {
            Ok(pubkey) => pubkey,
            Err(error) => {
                error!(%room_pubkey, %error, "refusing to observe invalid room pubkey");
                return;
            }
        };

        loop {
            if let Err(error) =
                observe_room_once(state.clone(), room_pubkey.clone(), parsed_room).await
            {
                warn!(%room_pubkey, %error, "room observer disconnected; retrying");
                sleep(Duration::from_secs(3)).await;
            }
        }
    });
}

async fn observe_room_once(state: AppState, room_pubkey: String, room: Pubkey) -> Result<()> {
    let (stream, _) = connect_async(state.config.solana_rpc_ws_url.as_str())
        .await
        .context("failed to connect to Solana pubsub")?;
    let (mut writer, mut reader) = stream.split();

    writer
        .send(Message::Text(
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "accountSubscribe",
                "params": [
                    room.to_string(),
                    {
                        "encoding": "base64",
                        "commitment": "confirmed"
                    }
                ]
            })
            .to_string()
            .into(),
        ))
        .await
        .context("failed to subscribe to account notifications")?;

    writer
        .send(Message::Text(
            json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "logsSubscribe",
                "params": [
                    {
                        "mentions": [room.to_string()]
                    },
                    {
                        "commitment": "confirmed"
                    }
                ]
            })
            .to_string()
            .into(),
        ))
        .await
        .context("failed to subscribe to logs notifications")?;

    info!(%room_pubkey, "room observer connected");

    while let Some(message) = reader.next().await {
        let message = message.context("room observer stream error")?;
        let text = match message {
            Message::Text(text) => text.to_string(),
            Message::Binary(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Message::Ping(_) | Message::Pong(_) => continue,
            Message::Close(_) => return Ok(()),
            Message::Frame(_) => continue,
        };

        let payload: Value = match serde_json::from_str(&text) {
            Ok(value) => value,
            Err(error) => {
                warn!(%room_pubkey, %error, "skipping invalid Solana pubsub payload");
                continue;
            }
        };

        let Some(method) = payload.get("method").and_then(Value::as_str) else {
            continue;
        };

        match method {
            "accountNotification" => {
                if let Some(result) = payload
                    .get("params")
                    .and_then(|params| params.get("result"))
                {
                    let slot = result
                        .get("context")
                        .and_then(|context| context.get("slot"))
                        .and_then(Value::as_u64)
                        .unwrap_or_default();
                    let account_value = result
                        .get("value")
                        .cloned()
                        .unwrap_or_else(|| Value::String("null".to_string()));
                    let encoded = serde_json::to_vec(&account_value).unwrap_or_default();
                    let hash = format!("{:x}", Sha256::digest(encoded));
                    state
                        .handle_room_account_update(&room_pubkey, hash, slot)
                        .await;
                }
            }
            "logsNotification" => {
                if let Some(result) = payload
                    .get("params")
                    .and_then(|params| params.get("result"))
                {
                    let slot = result
                        .get("context")
                        .and_then(|context| context.get("slot"))
                        .and_then(Value::as_u64)
                        .unwrap_or_default();
                    let value = result.get("value").cloned().unwrap_or(Value::Null);
                    let signature = value
                        .get("signature")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let logs = value
                        .get("logs")
                        .and_then(Value::as_array)
                        .map(|entries| {
                            entries
                                .iter()
                                .filter_map(Value::as_str)
                                .map(ToString::to_string)
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    state
                        .handle_room_log_update(&room_pubkey, signature, logs, slot)
                        .await;
                }
            }
            _ => {}
        }
    }

    Ok(())
}
