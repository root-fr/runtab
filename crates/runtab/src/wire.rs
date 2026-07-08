//! On-the-wire types for the sync API (`/v1`), coded against `docs/api/contract.md`.
//! `SyncedRecord` is the fail-closed whitelist: the client sends exactly these
//! fields and nothing else, so a raw prompt or file path can never be uploaded.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncedRecord {
    pub event_id: String,
    pub ts: String,
    pub agent: String,
    pub model: String,
    pub project_label: String,
    pub session_id: String,
    pub machine_id: String,
    pub machine_name: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub reasoning_tokens: u64,
    pub est_cost_microusd: u64,
    pub cost_basis: String,
}

#[derive(Debug, Serialize)]
pub struct EventBatch<'a> {
    pub events: &'a [SyncedRecord],
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Rejection {
    pub event_id: String,
    pub reason: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PushResult {
    pub accepted: u64,
    pub rejected: u64,
    #[serde(default)]
    pub rejections: Vec<Rejection>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PulledRecord {
    pub server_seq: i64,
    #[serde(flatten)]
    pub record: SyncedRecord,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PullResponse {
    pub events: Vec<PulledRecord>,
    pub next_since: i64,
    pub has_more: bool,
}

#[derive(Debug, Serialize)]
pub struct DeviceStartRequest<'a> {
    pub machine_name: &'a str,
    pub device_code: &'a str,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DeviceStartOk {
    pub request: String,
    pub display_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: String,
    pub interval_s: u64,
    pub expires_in_s: u64,
}

#[derive(Debug, Serialize)]
pub struct PollRequest<'a> {
    pub device_code: &'a str,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PollOk {
    pub token: String,
    pub user_id: String,
    pub email: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MachineInfo {
    pub machine_id: String,
    pub machine_name: String,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub last_seen: Option<String>,
    #[serde(default)]
    pub revoked: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MachinesResponse {
    pub machines: Vec<MachineInfo>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DeleteResult {
    pub deleted: bool,
    #[serde(default)]
    pub events_removed: u64,
    #[serde(default)]
    pub machines_removed: u64,
}
