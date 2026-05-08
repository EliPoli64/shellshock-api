use std::time::Duration;

use axum::{
    extract::{
        State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    response::IntoResponse,
};
use futures_util::{SinkExt, StreamExt};
use tokio::time::sleep;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::{info, warn};

use crate::{
    model::{ClientMessage, MatchRole, ServerMessage},
    state::{AppState, MatchLifecycle},
};

pub async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(state, socket))
}

async fn handle_socket(state: AppState, socket: WebSocket) {
    let (client_id, receiver) = state.register_client().await;
    let (mut sender, mut reader) = socket.split();

    let send_loop = tokio::spawn(async move {
        let mut outbound = UnboundedReceiverStream::new(receiver);
        while let Some(message) = outbound.next().await {
            let serialized = match serde_json::to_string(&message) {
                Ok(serialized) => serialized,
                Err(error) => {
                    warn!(%error, "failed to serialize relay message");
                    continue;
                }
            };

            if sender.send(Message::Text(serialized.into())).await.is_err() {
                break;
            }
        }
    });

    while let Some(frame) = reader.next().await {
        let frame = match frame {
            Ok(frame) => frame,
            Err(error) => {
                warn!(client_id = %client_id, %error, "websocket receive error");
                break;
            }
        };

        let payload = match frame {
            Message::Text(text) => text.to_string(),
            Message::Binary(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Message::Ping(_) | Message::Pong(_) => continue,
            Message::Close(_) => break,
        };

        let message = match serde_json::from_str::<ClientMessage>(&payload) {
            Ok(message) => message,
            Err(error) => {
                state
                    .send_to_client(
                        &client_id,
                        ServerMessage::SystemError {
                            code: "bad_message".to_string(),
                            message: format!("invalid websocket payload: {error}"),
                        },
                    )
                    .await;
                continue;
            }
        };

        handle_message(&state, &client_id, message).await;
    }

    send_loop.abort();
    state.unregister_client(&client_id).await;
}

async fn handle_message(state: &AppState, client_id: &str, message: ClientMessage) {
    match message {
        ClientMessage::QueueJoin {
            wallet,
            bet_lamports,
        } => match state
            .queue_join(client_id.to_string(), wallet.clone(), bet_lamports)
            .await
        {
            Ok(outcome) => {
                state.send_to_client(client_id, outcome.joined).await;
                state.send_to_client(client_id, outcome.status).await;

                if let Some(runtime) = outcome.matched {
                    state
                        .send_to_wallet(
                            &runtime.creator_wallet,
                            ServerMessage::MatchFound {
                                match_id: runtime.match_id.clone(),
                                role: MatchRole::Creator,
                                opponent_wallet: runtime.joiner_wallet.clone(),
                            },
                        )
                        .await;
                    state
                        .send_to_wallet(
                            &runtime.joiner_wallet,
                            ServerMessage::MatchFound {
                                match_id: runtime.match_id.clone(),
                                role: MatchRole::Joiner,
                                opponent_wallet: runtime.creator_wallet.clone(),
                            },
                        )
                        .await;

                    let timeout_state = state.clone();
                    let timeout_match_id = runtime.match_id.clone();
                    let timeout_seconds = state.config.turn_timeout_seconds;
                    tokio::spawn(async move {
                        sleep(Duration::from_secs(timeout_seconds)).await;
                        if let Some(expired) = timeout_state.expire_match(&timeout_match_id).await {
                            timeout_state
                                .send_to_wallet(
                                    &expired.creator_wallet,
                                    ServerMessage::SystemError {
                                        code: "match_expired".to_string(),
                                        message: "creator did not publish room in time".to_string(),
                                    },
                                )
                                .await;
                            timeout_state
                                .send_to_wallet(
                                    &expired.joiner_wallet,
                                    ServerMessage::SystemError {
                                        code: "match_expired".to_string(),
                                        message: "match expired before room creation".to_string(),
                                    },
                                )
                                .await;
                        }
                    });
                }
            }
            Err(code) => {
                state
                    .send_to_client(
                        client_id,
                        ServerMessage::SystemError {
                            code: code.clone(),
                            message: humanize_error(&code),
                        },
                    )
                    .await;
            }
        },
        ClientMessage::QueueLeave { ticket_id } => {
            if state.queue_leave(&ticket_id).await {
                state
                    .send_to_client(
                        client_id,
                        ServerMessage::QueueStatus {
                            ahead_count: 0,
                            same_bet_count: 0,
                        },
                    )
                    .await;
            } else {
                state
                    .send_to_client(
                        client_id,
                        ServerMessage::SystemError {
                            code: "ticket_not_found".to_string(),
                            message: "queue ticket not found".to_string(),
                        },
                    )
                    .await;
            }
        }
        ClientMessage::MatchRoomCreated {
            match_id,
            room_pubkey,
            signature,
        } => match state
            .register_room_creation(&match_id, room_pubkey.clone(), signature)
            .await
        {
            Ok(runtime) => {
                if runtime.state != MatchLifecycle::Expired {
                    state
                        .send_to_wallet(
                            &runtime.joiner_wallet,
                            ServerMessage::MatchRoomReady {
                                match_id: runtime.match_id.clone(),
                                room_pubkey: room_pubkey.clone(),
                            },
                        )
                        .await;
                    state.spawn_observer_if_needed(room_pubkey.clone()).await;
                    if let Some(snapshot) = state.current_room_snapshot(&room_pubkey).await {
                        state.broadcast_to_room(&room_pubkey, snapshot).await;
                    }
                }
            }
            Err(code) => {
                state
                    .send_to_client(
                        client_id,
                        ServerMessage::SystemError {
                            code: code.clone(),
                            message: humanize_error(&code),
                        },
                    )
                    .await;
            }
        },
        ClientMessage::RoomSubscribe { room_pubkey } => {
            let room = state
                .subscribe_room(client_id.to_string(), room_pubkey.clone(), None)
                .await;
            state.spawn_observer_if_needed(room_pubkey.clone()).await;
            state
                .send_to_client(
                    client_id,
                    ServerMessage::RoomState {
                        room_pubkey: room.room_pubkey,
                        phase: room.phase,
                        turn_wallet: room.turn_wallet,
                        updated_at: room.updated_at,
                        last_signature: room.last_signature,
                    },
                )
                .await;
        }
        ClientMessage::SessionResume {
            wallet,
            room_pubkey,
            match_id,
        } => {
            let messages = state
                .resume_session(client_id, &wallet, room_pubkey, match_id)
                .await;
            for message in messages {
                state.send_to_client(client_id, message).await;
            }
            info!(client_id = %client_id, wallet = %wallet, "relay session resumed");
        }
    }
}

fn humanize_error(code: &str) -> String {
    match code {
        "wallet_already_queued" => "this wallet is already waiting in queue".to_string(),
        "wallet_already_in_match" => "this wallet already has an active match".to_string(),
        "match_not_found" => "match not found on relay".to_string(),
        "match_expired" => "match expired before the room became ready".to_string(),
        _ => code.replace('_', " "),
    }
}
