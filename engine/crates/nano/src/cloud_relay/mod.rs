//! The STUN and TURN servers of an attempt, from DCG's `CloudRelay` service, cached: a STUN+TURN
//! answer for 85 % of its credential's remaining lifetime, a STUN-only answer for a day.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::Deserialize;
use tracing::debug;

use phonelink_core::seams::{TokenError, TokenSource};
use phonelink_core::sync::MutexExt;
use phonelink_core::types::{Environment, TraceContext};
use phonelink_transport::auth::dcg_rest::{DcgRest, RestError};

const STUN_ONLY_LIFETIME: Duration = Duration::from_secs(24 * 60 * 60);
const TURN_LIFETIME_SHARE: f64 = 0.85;

/// The servers `CloudRelay` returned, strings unchanged.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RelayServers {
    pub stun_urls: Vec<String>,
    pub turn_urls: Vec<String>,
    pub turn_username: String,
    pub turn_credential: String,
    /// Unix seconds.
    pub turn_expire_time: Option<i64>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct Urls {
    #[serde(default)]
    urls: Vec<String>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct TurnCredential {
    #[serde(default)]
    urls: Vec<String>,
    #[serde(default)]
    username: String,
    #[serde(default)]
    credential: String,
    turn_expire_time: Option<i64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Response {
    #[serde(default)]
    stun_info: Option<Urls>,
    #[serde(default)]
    turn_credential: Option<TurnCredential>,
}

impl RelayServers {
    pub fn from_json(value: &serde_json::Value) -> Result<Self, serde_json::Error> {
        let r: Response = serde_json::from_value(value.clone())?;
        let stun = r.stun_info.unwrap_or_default();
        let turn = r.turn_credential.unwrap_or_default();

        Ok(Self {
            stun_urls: stun.urls,
            turn_urls: turn.urls,
            turn_username: turn.username,
            turn_credential: turn.credential,
            turn_expire_time: turn.turn_expire_time,
        })
    }

    pub fn has_stun_and_turn(&self) -> bool {
        !self.stun_urls.is_empty() && !self.turn_urls.is_empty()
    }
}

/// Why no STUN/TURN servers were obtained.
#[derive(Debug, thiserror::Error)]
pub enum RelayServersError {
    #[error("no relay source")]
    NoSource,
    #[error(transparent)]
    Token(#[from] TokenError),
    #[error(transparent)]
    Rest(#[from] RestError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

/// Where an attempt's STUN/TURN servers come from.
#[async_trait]
pub trait RelaySource: Send + Sync {
    async fn relay_servers(&self, only_stun: bool) -> Result<RelayServers, RelayServersError>;
}

/// No servers at all: attempts that need them are skipped.
pub struct NoRelay;

#[async_trait]
impl RelaySource for NoRelay {
    async fn relay_servers(&self, _only_stun: bool) -> Result<RelayServers, RelayServersError> {
        Err(RelayServersError::NoSource)
    }
}

/// `CloudRelay` through the DCG REST client and the services token.
pub struct CloudRelay {
    rest: DcgRest,
    tokens: Arc<dyn TokenSource>,
    env: Environment,
    cache: Mutex<HashMap<bool, (RelayServers, Instant)>>,
}

impl CloudRelay {
    pub fn new(rest: DcgRest, tokens: Arc<dyn TokenSource>, env: Environment) -> Self {
        Self { rest, tokens, env, cache: Mutex::default() }
    }

    fn lifetime(only_stun: bool, servers: &RelayServers) -> Option<Duration> {
        if only_stun {
            return Some(STUN_ONLY_LIFETIME);
        }

        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
        let left = servers.turn_expire_time? - now;
        (left > 0).then(|| Duration::from_secs_f64(TURN_LIFETIME_SHARE * left as f64))
    }
}

#[async_trait]
impl RelaySource for CloudRelay {
    async fn relay_servers(&self, only_stun: bool) -> Result<RelayServers, RelayServersError> {
        if let Some((servers, until)) = self.cache.lock_or_panic().get(&only_stun) {
            if Instant::now() < *until {
                return Ok(servers.clone());
            }
        }

        let token = self.tokens.services_token(self.env, false).await?;
        let trace = TraceContext::new_root(Vec::new());
        let value = self.rest.cloud_relay(self.env, &token, only_stun, &trace).await?;
        let servers = RelayServers::from_json(&value)?;
        debug!(
            only_stun,
            stun_count = servers.stun_urls.len(),
            turn_count = servers.turn_urls.len(),
            "CloudRelay servers received"
        );

        if let Some(lifetime) = Self::lifetime(only_stun, &servers) {
            self.cache.lock_or_panic().insert(only_stun, (servers.clone(), Instant::now() + lifetime));
        }
        Ok(servers)
    }
}

#[cfg(test)]
mod tests;
