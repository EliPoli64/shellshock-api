use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::Arc;

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde_json::json;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::commitment_config::CommitmentConfig;
use tokio::sync::{Mutex, RwLock, mpsc};
use tracing::warn;
use uuid::Uuid;

use crate::config::Config;
use crate::model::{MatchRole, ReadyzResponse, RoomPhase, ServerMessage};
use crate::solana::spawn_room_observer;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    rpc_client: Arc<RpcClient>,
    runtime: Arc<RuntimeState>,
}

struct RuntimeState {
    clients: RwLock<HashMap<String, mpsc::UnboundedSender<ServerMessage>>>,
    wallet_bindings: RwLock<HashMap<String, String>>,
    queue: Mutex<HashMap<u64, VecDeque<QueueEntry>>>,
    matches: Mutex<HashMap<String, MatchRuntime>>,
    rooms: Mutex<HashMap<String, RoomRuntime>>,
    wallet_sessions: Mutex<HashMap<String, WalletSession>>,
    observers: Mutex<HashSet<String>>,
}

#[derive(Clone)]
pub struct QueueJoinOutcome {
    pub joined: ServerMessage,
    pub status: ServerMessage,
    pub matched: Option<MatchRuntime>,
}

#[derive(Clone)]
pub struct QueueEntry {
    pub ticket_id: String,
    pub wallet: String,
    pub client_id: String,
    pub bet_lamports: u64,
    pub queued_at: DateTime<Utc>,
}

#[derive(Clone)]
pub struct MatchRuntime {
    pub match_id: String,
    pub bet_lamports: u64,
    pub creator_wallet: String,
    pub creator_client: String,
    pub joiner_wallet: String,
    pub joiner_client: String,
    pub room_pubkey: Option<String>,
    pub state: MatchLifecycle,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, PartialEq, Eq)]
pub enum MatchLifecycle {
    AwaitingRoomCreation,
    RoomReady,
    Expired,
}

#[derive(Clone)]
pub struct RoomRuntime {
    pub room_pubkey: String,
    pub match_id: Option<String>,
    pub wallets: BTreeSet<String>,
    pub subscribers: BTreeSet<String>,
    pub phase: RoomPhase,
    pub turn_wallet: Option<String>,
    pub updated_at: DateTime<Utc>,
    pub last_signature: Option<String>,
    pub last_account_hash: Option<String>,
}

#[derive(Clone, Default)]
struct WalletSession {
    ticket_id: Option<String>,
    current_match_id: Option<String>,
    current_room_pubkey: Option<String>,
}

impl AppState {
    pub async fn new(config: Arc<Config>) -> Result<Self> {
        let rpc_client = Arc::new(RpcClient::new_with_commitment(
            config.solana_rpc_http_url.clone(),
            CommitmentConfig::confirmed(),
        ));
        Ok(Self {
            config,
            rpc_client,
            runtime: Arc::new(RuntimeState {
                clients: RwLock::new(HashMap::new()),
                wallet_bindings: RwLock::new(HashMap::new()),
                queue: Mutex::new(HashMap::new()),
                matches: Mutex::new(HashMap::new()),
                rooms: Mutex::new(HashMap::new()),
                wallet_sessions: Mutex::new(HashMap::new()),
                observers: Mutex::new(HashSet::new()),
            }),
        })
    }

    pub async fn register_client(&self) -> (String, mpsc::UnboundedReceiver<ServerMessage>) {
        let client_id = Uuid::new_v4().to_string();
        let (sender, receiver) = mpsc::unbounded_channel();
        self.runtime
            .clients
            .write()
            .await
            .insert(client_id.clone(), sender);
        (client_id, receiver)
    }

    pub async fn unregister_client(&self, client_id: &str) {
        self.runtime.clients.write().await.remove(client_id);

        let mut rooms = self.runtime.rooms.lock().await;
        for room in rooms.values_mut() {
            room.subscribers.remove(client_id);
        }
    }

    pub async fn bind_wallet(&self, client_id: &str, wallet: &str) {
        self.runtime
            .wallet_bindings
            .write()
            .await
            .insert(wallet.to_string(), client_id.to_string());
    }

    pub async fn send_to_client(&self, client_id: &str, message: ServerMessage) {
        if let Some(sender) = self.runtime.clients.read().await.get(client_id) {
            let _ = sender.send(message);
        }
    }

    pub async fn send_to_wallet(&self, wallet: &str, message: ServerMessage) {
        let client_id = self
            .runtime
            .wallet_bindings
            .read()
            .await
            .get(wallet)
            .cloned();
        if let Some(client_id) = client_id {
            self.send_to_client(&client_id, message).await;
        }
    }

    pub async fn queue_join(
        &self,
        client_id: String,
        wallet: String,
        bet_lamports: u64,
    ) -> std::result::Result<QueueJoinOutcome, String> {
        self.bind_wallet(&client_id, &wallet).await;

        {
            let queue = self.runtime.queue.lock().await;
            if queue
                .values()
                .any(|entries| entries.iter().any(|entry| entry.wallet == wallet))
            {
                return Err("wallet_already_queued".to_string());
            }
        }

        {
            let matches = self.runtime.matches.lock().await;
            if matches.values().any(|runtime| {
                runtime.state != MatchLifecycle::Expired
                    && (runtime.creator_wallet == wallet || runtime.joiner_wallet == wallet)
            }) {
                return Err("wallet_already_in_match".to_string());
            }
        }

        let queued_at = Utc::now();
        let ticket_id = Uuid::new_v4().to_string();
        let joined = ServerMessage::QueueJoined {
            ticket_id: ticket_id.clone(),
            bet_lamports,
            queued_at,
        };
        let entry = QueueEntry {
            ticket_id: ticket_id.clone(),
            wallet: wallet.clone(),
            client_id: client_id.clone(),
            bet_lamports,
            queued_at,
        };

        let mut queue = self.runtime.queue.lock().await;
        let mut matches = self.runtime.matches.lock().await;
        let mut wallet_sessions = self.runtime.wallet_sessions.lock().await;

        let bucket = queue.entry(bet_lamports).or_default();
        let ahead_count = bucket.len();

        if let Some(creator) = bucket.pop_front() {
            let match_id = Uuid::new_v4().to_string();
            let runtime = MatchRuntime {
                match_id: match_id.clone(),
                bet_lamports,
                creator_wallet: creator.wallet.clone(),
                creator_client: creator.client_id.clone(),
                joiner_wallet: wallet.clone(),
                joiner_client: client_id,
                room_pubkey: None,
                state: MatchLifecycle::AwaitingRoomCreation,
                created_at: Utc::now(),
            };

            matches.insert(match_id.clone(), runtime.clone());

            wallet_sessions
                .entry(creator.wallet.clone())
                .or_default()
                .ticket_id = None;
            wallet_sessions
                .entry(creator.wallet.clone())
                .or_default()
                .current_match_id = Some(match_id.clone());
            wallet_sessions.entry(wallet.clone()).or_default().ticket_id = None;
            wallet_sessions.entry(wallet).or_default().current_match_id = Some(match_id.clone());

            Ok(QueueJoinOutcome {
                joined,
                status: ServerMessage::QueueStatus {
                    ahead_count: 0,
                    same_bet_count: 0,
                },
                matched: Some(runtime),
            })
        } else {
            bucket.push_back(entry);
            wallet_sessions.entry(wallet).or_default().ticket_id = Some(ticket_id);

            Ok(QueueJoinOutcome {
                joined,
                status: ServerMessage::QueueStatus {
                    ahead_count,
                    same_bet_count: bucket.len(),
                },
                matched: None,
            })
        }
    }

    pub async fn queue_leave(&self, ticket_id: &str) -> bool {
        let mut queue = self.runtime.queue.lock().await;
        let mut removed_wallet = None;

        for entries in queue.values_mut() {
            if let Some(index) = entries
                .iter()
                .position(|entry| entry.ticket_id == ticket_id)
            {
                let removed = entries.remove(index).expect("queue index must exist");
                removed_wallet = Some(removed.wallet);
                break;
            }
        }

        if let Some(wallet) = removed_wallet {
            if let Some(session) = self.runtime.wallet_sessions.lock().await.get_mut(&wallet) {
                session.ticket_id = None;
            }
            true
        } else {
            false
        }
    }

    pub async fn register_room_creation(
        &self,
        match_id: &str,
        room_pubkey: String,
        signature: String,
    ) -> std::result::Result<MatchRuntime, String> {
        let mut matches = self.runtime.matches.lock().await;
        let mut rooms = self.runtime.rooms.lock().await;
        let mut wallet_sessions = self.runtime.wallet_sessions.lock().await;

        let runtime = matches
            .get_mut(match_id)
            .ok_or_else(|| "match_not_found".to_string())?;

        if runtime.state == MatchLifecycle::Expired {
            return Err("match_expired".to_string());
        }

        runtime.room_pubkey = Some(room_pubkey.clone());
        runtime.state = MatchLifecycle::RoomReady;

        let room = rooms
            .entry(room_pubkey.clone())
            .or_insert_with(|| RoomRuntime {
                room_pubkey: room_pubkey.clone(),
                match_id: Some(match_id.to_string()),
                wallets: BTreeSet::new(),
                subscribers: BTreeSet::new(),
                phase: RoomPhase::WaitingForPlayer,
                turn_wallet: None,
                updated_at: Utc::now(),
                last_signature: None,
                last_account_hash: None,
            });

        room.match_id = Some(match_id.to_string());
        room.wallets.insert(runtime.creator_wallet.clone());
        room.wallets.insert(runtime.joiner_wallet.clone());
        room.phase = RoomPhase::WaitingForPlayer;
        room.updated_at = Utc::now();
        room.last_signature = Some(signature);

        wallet_sessions
            .entry(runtime.creator_wallet.clone())
            .or_default()
            .current_room_pubkey = Some(room_pubkey.clone());
        wallet_sessions
            .entry(runtime.joiner_wallet.clone())
            .or_default()
            .current_room_pubkey = Some(room_pubkey);

        Ok(runtime.clone())
    }

    pub async fn subscribe_room(
        &self,
        client_id: String,
        room_pubkey: String,
        wallet: Option<String>,
    ) -> RoomRuntime {
        let mut rooms = self.runtime.rooms.lock().await;
        let room = rooms
            .entry(room_pubkey.clone())
            .or_insert_with(|| RoomRuntime {
                room_pubkey: room_pubkey.clone(),
                match_id: None,
                wallets: BTreeSet::new(),
                subscribers: BTreeSet::new(),
                phase: RoomPhase::WaitingForPlayer,
                turn_wallet: None,
                updated_at: Utc::now(),
                last_signature: None,
                last_account_hash: None,
            });
        room.subscribers.insert(client_id);
        if let Some(wallet) = wallet {
            room.wallets.insert(wallet);
        }
        room.clone()
    }

    pub async fn current_room_snapshot(&self, room_pubkey: &str) -> Option<ServerMessage> {
        let rooms = self.runtime.rooms.lock().await;
        rooms.get(room_pubkey).map(|room| ServerMessage::RoomState {
            room_pubkey: room.room_pubkey.clone(),
            phase: room.phase.clone(),
            turn_wallet: room.turn_wallet.clone(),
            updated_at: room.updated_at,
            last_signature: room.last_signature.clone(),
        })
    }

    pub async fn spawn_observer_if_needed(&self, room_pubkey: String) {
        let mut observers = self.runtime.observers.lock().await;
        if observers.insert(room_pubkey.clone()) {
            spawn_room_observer(self.clone(), room_pubkey).await;
        }
    }

    pub async fn resume_session(
        &self,
        client_id: &str,
        wallet: &str,
        room_pubkey: Option<String>,
        match_id: Option<String>,
    ) -> Vec<ServerMessage> {
        self.bind_wallet(client_id, wallet).await;

        let mut messages = Vec::new();

        let session = self
            .runtime
            .wallet_sessions
            .lock()
            .await
            .get(wallet)
            .cloned()
            .unwrap_or_default();

        if let Some(ticket_id) = session.ticket_id {
            let queue = self.runtime.queue.lock().await;
            if let Some(entry) = queue
                .values()
                .flat_map(|entries| entries.iter())
                .find(|entry| entry.ticket_id == ticket_id)
            {
                let ahead_count = queue
                    .get(&entry.bet_lamports)
                    .map(|entries| {
                        entries
                            .iter()
                            .position(|queued| queued.ticket_id == entry.ticket_id)
                            .unwrap_or_default()
                    })
                    .unwrap_or_default();
                let same_bet_count = queue
                    .get(&entry.bet_lamports)
                    .map(VecDeque::len)
                    .unwrap_or_default();
                messages.push(ServerMessage::QueueJoined {
                    ticket_id: entry.ticket_id.clone(),
                    bet_lamports: entry.bet_lamports,
                    queued_at: entry.queued_at,
                });
                messages.push(ServerMessage::QueueStatus {
                    ahead_count,
                    same_bet_count,
                });
            }
        }

        let preferred_match_id = match_id.or(session.current_match_id);
        if let Some(match_id) = preferred_match_id {
            let matches = self.runtime.matches.lock().await;
            if let Some(runtime) = matches.get(&match_id) {
                let (role, opponent_wallet) = if runtime.creator_wallet == wallet {
                    (MatchRole::Creator, runtime.joiner_wallet.clone())
                } else {
                    (MatchRole::Joiner, runtime.creator_wallet.clone())
                };
                messages.push(ServerMessage::MatchFound {
                    match_id: runtime.match_id.clone(),
                    role,
                    opponent_wallet,
                });
                if let Some(room_pubkey) = &runtime.room_pubkey {
                    messages.push(ServerMessage::MatchRoomReady {
                        match_id: runtime.match_id.clone(),
                        room_pubkey: room_pubkey.clone(),
                    });
                }
            }
        }

        let preferred_room_pubkey = room_pubkey.or(session.current_room_pubkey);
        if let Some(room_pubkey) = preferred_room_pubkey {
            let room = self
                .subscribe_room(
                    client_id.to_string(),
                    room_pubkey.clone(),
                    Some(wallet.to_string()),
                )
                .await;
            messages.push(ServerMessage::RoomState {
                room_pubkey: room.room_pubkey.clone(),
                phase: room.phase,
                turn_wallet: room.turn_wallet.clone(),
                updated_at: room.updated_at,
                last_signature: room.last_signature.clone(),
            });
            self.spawn_observer_if_needed(room_pubkey).await;
        }

        messages
    }

    pub async fn expire_match(&self, match_id: &str) -> Option<MatchRuntime> {
        let mut matches = self.runtime.matches.lock().await;
        let runtime = matches.get_mut(match_id)?;
        if runtime.state != MatchLifecycle::AwaitingRoomCreation {
            return None;
        }
        runtime.state = MatchLifecycle::Expired;
        Some(runtime.clone())
    }

    pub async fn readiness(&self) -> ReadyzResponse {
        let program_id_valid = self.config.validate_program_id().is_ok();
        let rpc_reachable = self.rpc_client.get_latest_blockhash().await.is_ok();
        let observed_rooms = self.runtime.rooms.lock().await.len();
        let queued_players = self
            .runtime
            .queue
            .lock()
            .await
            .values()
            .map(VecDeque::len)
            .sum();
        let active_matches = self
            .runtime
            .matches
            .lock()
            .await
            .values()
            .filter(|runtime| runtime.state != MatchLifecycle::Expired)
            .count();

        ReadyzResponse {
            status: if program_id_valid && rpc_reachable {
                "ok"
            } else {
                "degraded"
            },
            program_id_valid,
            rpc_reachable,
            observed_rooms,
            queued_players,
            active_matches,
        }
    }

    pub async fn handle_room_account_update(
        &self,
        room_pubkey: &str,
        account_hash: String,
        slot: u64,
    ) {
        let snapshot = {
            let mut rooms = self.runtime.rooms.lock().await;
            let Some(room) = rooms.get_mut(room_pubkey) else {
                return;
            };

            if room.last_account_hash.as_ref() == Some(&account_hash) {
                None
            } else {
                room.last_account_hash = Some(account_hash.clone());
                room.updated_at = Utc::now();
                room.phase = promote_phase_from_account(&room.phase);
                Some(ServerMessage::RoomState {
                    room_pubkey: room.room_pubkey.clone(),
                    phase: room.phase.clone(),
                    turn_wallet: room.turn_wallet.clone(),
                    updated_at: room.updated_at,
                    last_signature: room.last_signature.clone(),
                })
            }
        };

        self.broadcast_to_room(
            room_pubkey,
            ServerMessage::RoomEvent {
                room_pubkey: room_pubkey.to_string(),
                event_type: "account_change".to_string(),
                payload: json!({
                    "slot": slot,
                    "account_hash": account_hash,
                }),
            },
        )
        .await;

        if let Some(snapshot) = snapshot {
            self.broadcast_to_room(room_pubkey, snapshot).await;
        }
    }

    pub async fn handle_room_log_update(
        &self,
        room_pubkey: &str,
        signature: String,
        logs: Vec<String>,
        slot: u64,
    ) {
        let snapshot = {
            let mut rooms = self.runtime.rooms.lock().await;
            let Some(room) = rooms.get_mut(room_pubkey) else {
                return;
            };

            room.updated_at = Utc::now();
            room.last_signature = Some(signature.clone());
            if let Some(phase) = infer_phase_from_logs(&room.phase, &logs) {
                room.phase = phase;
            }

            Some(ServerMessage::RoomState {
                room_pubkey: room.room_pubkey.clone(),
                phase: room.phase.clone(),
                turn_wallet: room.turn_wallet.clone(),
                updated_at: room.updated_at,
                last_signature: room.last_signature.clone(),
            })
        };

        self.broadcast_to_room(
            room_pubkey,
            ServerMessage::RoomEvent {
                room_pubkey: room_pubkey.to_string(),
                event_type: "program_logs".to_string(),
                payload: json!({
                    "slot": slot,
                    "signature": signature,
                    "logs": logs,
                }),
            },
        )
        .await;

        if let Some(snapshot) = snapshot {
            self.broadcast_to_room(room_pubkey, snapshot).await;
        }
    }

    pub async fn broadcast_to_room(&self, room_pubkey: &str, message: ServerMessage) {
        let (wallets, subscribers) = {
            let rooms = self.runtime.rooms.lock().await;
            let Some(room) = rooms.get(room_pubkey) else {
                return;
            };
            (room.wallets.clone(), room.subscribers.clone())
        };

        let wallet_bindings = self.runtime.wallet_bindings.read().await;
        let mut recipients = BTreeSet::new();
        for wallet in wallets {
            if let Some(client_id) = wallet_bindings.get(&wallet) {
                recipients.insert(client_id.clone());
            }
        }
        drop(wallet_bindings);

        recipients.extend(subscribers);

        for client_id in recipients {
            self.send_to_client(&client_id, message.clone()).await;
        }
    }
}

fn infer_phase_from_logs(current: &RoomPhase, logs: &[String]) -> Option<RoomPhase> {
    let normalized = logs.join("\n").to_lowercase();
    if normalized.contains("finished") || normalized.contains("winner") {
        return Some(RoomPhase::Finished);
    }
    if normalized.contains("round end") || normalized.contains("round_end") {
        return Some(RoomPhase::RoundEnd);
    }
    if normalized.contains("vrf") || normalized.contains("randomness") {
        return Some(RoomPhase::WaitingForVrf);
    }
    if normalized.contains("shoot")
        || normalized.contains("turn")
        || normalized.contains("reload")
        || normalized.contains("item")
    {
        return Some(RoomPhase::Playing);
    }

    if *current == RoomPhase::WaitingForPlayer {
        Some(RoomPhase::WaitingForVrf)
    } else {
        None
    }
}

fn promote_phase_from_account(current: &RoomPhase) -> RoomPhase {
    match current {
        RoomPhase::WaitingForPlayer => RoomPhase::WaitingForVrf,
        RoomPhase::WaitingForVrf => RoomPhase::Playing,
        phase => phase.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn queue_matches_same_bet() {
        let config = Arc::new(Config {
            port: 8080,
            solana_rpc_http_url: "http://localhost:8899".to_string(),
            solana_rpc_ws_url: "ws://localhost:8900".to_string(),
            program_id: "11111111111111111111111111111111".to_string(),
            cors_origin: "http://localhost:5173".to_string(),
            turn_timeout_seconds: 90,
        });
        let state = AppState::new(config).await.expect("state");

        let first = state
            .queue_join("client-a".to_string(), "wallet-a".to_string(), 1_000_000)
            .await
            .expect("first queue join");
        assert!(first.matched.is_none());

        let second = state
            .queue_join("client-b".to_string(), "wallet-b".to_string(), 1_000_000)
            .await
            .expect("second queue join");
        assert!(second.matched.is_some());
    }

    #[tokio::test]
    async fn queue_separates_different_bets() {
        let config = Arc::new(Config {
            port: 8080,
            solana_rpc_http_url: "http://localhost:8899".to_string(),
            solana_rpc_ws_url: "ws://localhost:8900".to_string(),
            program_id: "11111111111111111111111111111111".to_string(),
            cors_origin: "http://localhost:5173".to_string(),
            turn_timeout_seconds: 90,
        });
        let state = AppState::new(config).await.expect("state");

        let first = state
            .queue_join("client-a".to_string(), "wallet-a".to_string(), 1_000_000)
            .await
            .expect("first queue join");
        let second = state
            .queue_join("client-b".to_string(), "wallet-b".to_string(), 2_000_000)
            .await
            .expect("second queue join");

        assert!(first.matched.is_none());
        assert!(second.matched.is_none());
    }
}
