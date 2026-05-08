use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RoomPhase {
    WaitingForPlayer,
    WaitingForVrf,
    Playing,
    RoundEnd,
    Finished,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum ClientMessage {
    #[serde(rename = "queue.join")]
    QueueJoin { wallet: String, bet_lamports: u64 },
    #[serde(rename = "queue.leave")]
    QueueLeave { ticket_id: String },
    #[serde(rename = "match.room_created")]
    MatchRoomCreated {
        match_id: String,
        room_pubkey: String,
        signature: String,
    },
    #[serde(rename = "room.subscribe")]
    RoomSubscribe { room_pubkey: String },
    #[serde(rename = "session.resume")]
    SessionResume {
        wallet: String,
        room_pubkey: Option<String>,
        match_id: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum ServerMessage {
    #[serde(rename = "queue.joined")]
    QueueJoined {
        ticket_id: String,
        bet_lamports: u64,
        queued_at: DateTime<Utc>,
    },
    #[serde(rename = "queue.status")]
    QueueStatus {
        ahead_count: usize,
        same_bet_count: usize,
    },
    #[serde(rename = "match.found")]
    MatchFound {
        match_id: String,
        role: MatchRole,
        opponent_wallet: String,
    },
    #[serde(rename = "match.room_ready")]
    MatchRoomReady {
        match_id: String,
        room_pubkey: String,
    },
    #[serde(rename = "room.state")]
    RoomState {
        room_pubkey: String,
        phase: RoomPhase,
        turn_wallet: Option<String>,
        updated_at: DateTime<Utc>,
        last_signature: Option<String>,
    },
    #[serde(rename = "room.event")]
    RoomEvent {
        room_pubkey: String,
        event_type: String,
        payload: Value,
    },
    #[serde(rename = "system.error")]
    SystemError { code: String, message: String },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MatchRole {
    Creator,
    Joiner,
}

#[derive(Debug, Clone, Serialize)]
pub struct HealthzResponse {
    pub status: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReadyzResponse {
    pub status: &'static str,
    pub program_id_valid: bool,
    pub rpc_reachable: bool,
    pub observed_rooms: usize,
    pub queued_players: usize,
    pub active_matches: usize,
}
