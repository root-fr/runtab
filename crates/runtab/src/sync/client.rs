use std::fmt;
use std::time::Duration;

use reqwest::{Client, StatusCode};

use crate::wire::{
    DeleteResult, DeviceStartOk, DeviceStartRequest, EventBatch, MachineInfo, MachinesResponse,
    PollOk, PollRequest, PullResponse, PushResult, SyncedRecord,
};

#[derive(Debug)]
pub enum SyncError {
    /// The server rejected the bearer token (revoked or deleted account).
    Unauthorized,
    Server(u16, String),
    Network(String),
    Decode(String),
    /// A local ledger or lock failure while preparing/applying a sync.
    Local(String),
    /// The per-minute `EventsPost` window is full; `retry_after` is seconds.
    RateLimited { retry_after: u64 },
    /// The account's daily ingest quota is reached; `retry_after` is seconds
    /// until UTC midnight.
    QuotaDaily { retry_after: u64 },
}

impl fmt::Display for SyncError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SyncError::Unauthorized => write!(f, "sync token rejected (401)"),
            SyncError::Server(c, m) => write!(f, "server error {c}: {m}"),
            SyncError::Network(m) => write!(f, "network error: {m}"),
            SyncError::Decode(m) => write!(f, "decode error: {m}"),
            SyncError::Local(m) => write!(f, "local error: {m}"),
            SyncError::RateLimited { retry_after } => write!(f, "rate limited, retry after {retry_after}s"),
            SyncError::QuotaDaily { retry_after } => write!(f, "daily quota reached, retry after {retry_after}s"),
        }
    }
}

impl std::error::Error for SyncError {}

type R<T> = Result<T, SyncError>;

pub enum PollOutcome {
    Confirmed { token: String, user_id: String, email: String },
    Pending,
    Expired,
    /// The approving account already has `MAX_MACHINES` active machines.
    MachineLimit,
}

pub struct SyncClient {
    base: String,
    http: Client,
}

impl SyncClient {
    pub fn new(base_url: &str) -> R<SyncClient> {
        // The preview sync server is reached directly (local docker / loopback).
        // Bypass any ambient HTTP(S)_PROXY so a proxy that intercepts localhost
        // cannot break sync.
        let http = Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|e| SyncError::Network(e.to_string()))?;
        Ok(SyncClient {
            base: base_url.trim_end_matches('/').to_string(),
            http,
        })
    }

    pub async fn healthz(&self) -> R<bool> {
        let resp = self.get(&format!("{}/healthz", self.base), None).await?;
        Ok(resp.status().is_success())
    }

    pub async fn device_start(&self, machine_name: &str, device_code: &str) -> R<DeviceStartOk> {
        let resp = self
            .http
            .post(format!("{}/v1/auth/device/start", self.base))
            .json(&DeviceStartRequest { machine_name, device_code })
            .send()
            .await
            .map_err(net)?;
        if resp.status().is_success() {
            resp.json().await.map_err(dec)
        } else {
            Err(status_err(resp).await)
        }
    }

    pub async fn auth_poll(&self, device_code: &str) -> R<PollOutcome> {
        let resp = self
            .http
            .post(format!("{}/v1/auth/poll", self.base))
            .json(&PollRequest { device_code })
            .send()
            .await
            .map_err(net)?;
        match resp.status() {
            StatusCode::OK => {
                let ok: PollOk = resp.json().await.map_err(dec)?;
                Ok(PollOutcome::Confirmed { token: ok.token, user_id: ok.user_id, email: ok.email })
            }
            StatusCode::PRECONDITION_REQUIRED => Ok(PollOutcome::Pending),
            StatusCode::GONE => Ok(PollOutcome::Expired),
            StatusCode::FORBIDDEN => {
                let body = resp.text().await.unwrap_or_default();
                if error_code(&body).as_deref() == Some("quota_machines") {
                    Ok(PollOutcome::MachineLimit)
                } else {
                    Err(SyncError::Server(403, body))
                }
            }
            _ => Err(status_err(resp).await),
        }
    }

    pub async fn push_events(&self, token: &str, records: &[SyncedRecord]) -> R<PushResult> {
        let resp = self
            .http
            .post(format!("{}/v1/events", self.base))
            .bearer_auth(token)
            .json(&EventBatch { events: records })
            .send()
            .await
            .map_err(net)?;
        if resp.status().is_success() {
            resp.json().await.map_err(dec)
        } else {
            Err(status_err(resp).await)
        }
    }

    pub async fn pull_events(&self, token: &str, since: i64, exclude_machine: &str) -> R<PullResponse> {
        let url = format!(
            "{}/v1/events?since={since}&exclude_machine={exclude_machine}",
            self.base
        );
        let resp = self.get(&url, Some(token)).await?;
        if resp.status().is_success() {
            resp.json().await.map_err(dec)
        } else {
            Err(status_err(resp).await)
        }
    }

    pub async fn machines(&self, token: &str) -> R<Vec<MachineInfo>> {
        let resp = self.get(&format!("{}/v1/machines", self.base), Some(token)).await?;
        if resp.status().is_success() {
            let m: MachinesResponse = resp.json().await.map_err(dec)?;
            Ok(m.machines)
        } else {
            Err(status_err(resp).await)
        }
    }

    pub async fn delete_account(&self, token: &str) -> R<DeleteResult> {
        let resp = self
            .http
            .delete(format!("{}/v1/account", self.base))
            .bearer_auth(token)
            .send()
            .await
            .map_err(net)?;
        if resp.status().is_success() {
            resp.json().await.map_err(dec)
        } else {
            Err(status_err(resp).await)
        }
    }

    async fn get(&self, url: &str, token: Option<&str>) -> R<reqwest::Response> {
        let mut req = self.http.get(url);
        if let Some(t) = token {
            req = req.bearer_auth(t);
        }
        req.send().await.map_err(net)
    }
}

fn net(e: reqwest::Error) -> SyncError {
    use std::error::Error;
    let mut msg = e.to_string();
    let mut src = e.source();
    while let Some(s) = src {
        msg.push_str(": ");
        msg.push_str(&s.to_string());
        src = s.source();
    }
    SyncError::Network(msg)
}

fn dec(e: reqwest::Error) -> SyncError {
    SyncError::Decode(e.to_string())
}

async fn status_err(resp: reqwest::Response) -> SyncError {
    let code = resp.status().as_u16();
    if code == 401 {
        return SyncError::Unauthorized;
    }
    if code == 429 {
        let retry_after = resp
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok());
        let body = resp.text().await.unwrap_or_default();
        return classify_throttle(retry_after, &body);
    }
    let body = resp.text().await.unwrap_or_default();
    SyncError::Server(code, body)
}

/// Extracts the `error` code from a JSON error body, if any.
fn error_code(body: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string))
}

/// Map a 429 into a structured throttle error. `retry_after` is the parsed
/// Retry-After header (seconds) if present; `body` is the response body.
pub(crate) fn classify_throttle(retry_after: Option<u64>, body: &str) -> SyncError {
    match error_code(body).as_deref() {
        Some("quota_daily") => SyncError::QuotaDaily { retry_after: retry_after.unwrap_or(3600) },
        _ => SyncError::RateLimited { retry_after: retry_after.unwrap_or(5) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_throttle_rate_limited_with_header() {
        let e = classify_throttle(Some(12), r#"{"error":"rate_limited"}"#);
        assert!(matches!(e, SyncError::RateLimited { retry_after: 12 }));
    }

    #[test]
    fn classify_throttle_quota_daily_with_header() {
        let e = classify_throttle(Some(30), r#"{"error":"quota_daily"}"#);
        assert!(matches!(e, SyncError::QuotaDaily { retry_after: 30 }));
    }

    #[test]
    fn classify_throttle_defaults_rate_limited_on_empty_body() {
        let e = classify_throttle(None, "");
        assert!(matches!(e, SyncError::RateLimited { retry_after: 5 }));
    }

    #[test]
    fn classify_throttle_defaults_rate_limited_on_garbage_body() {
        let e = classify_throttle(None, "not json{{{");
        assert!(matches!(e, SyncError::RateLimited { retry_after: 5 }));
    }

    #[test]
    fn classify_throttle_quota_daily_defaults_to_midnight_window() {
        let e = classify_throttle(None, r#"{"error":"quota_daily"}"#);
        assert!(matches!(e, SyncError::QuotaDaily { retry_after: 3600 }));
    }
}
