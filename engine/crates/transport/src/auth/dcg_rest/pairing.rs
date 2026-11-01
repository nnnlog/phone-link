//! `CloudRelay` and the QR/PIN pairing sessions.

use super::{DcgAuth, DcgRest, RestError, query_value};
use crate::auth::rest_policy::ServiceClient;
use phonelink_core::constants::{api_version, http_header as h};
use phonelink_core::types::{Environment, TraceContext};

impl DcgRest {
    /// `GET /CloudRelay/Relay?api-version=1.0.0&isOnlyStun=<bool>` with the DCG services bearer →
    /// the STUN servers and, unless `only_stun`, the TURN servers and credential. The body is
    /// returned as the service wrote it.
    pub async fn cloud_relay(
        &self,
        env: Environment,
        services: &str,
        only_stun: bool,
        trace: &TraceContext,
    ) -> Result<serde_json::Value, RestError> {
        const CALL: &str = "CloudRelay";
        let url = Self::url(
            self.base(env),
            &format!("CloudRelay/Relay?api-version={}&isOnlyStun={only_stun}", api_version::CLOUD_RELAY),
            CALL,
        )?;

        let reply = self
            .send(
                CALL,
                ServiceClient::CloudRelay,
                reqwest::Method::GET,
                env.name(),
                url,
                DcgAuth::Services(services),
                trace,
                None,
            )
            .await?;

        Self::parse(CALL, &reply)
    }

    /// `POST /DevicePairingProxy/PairingSessions?Api-Version=3.0.0` with `body` (the create request's
    /// JSON) and `clientRegionName` when `region` is given → the 2xx reply's body as sent.
    pub async fn create_pairing_session(
        &self,
        env: Environment,
        msa: &str,
        region: Option<&str>,
        body: String,
        trace: &TraceContext,
    ) -> Result<String, RestError> {
        const CALL: &str = "CreatePairingSession";
        let url = Self::url(self.base(env), &Self::pairing_path("", ""), CALL)?;
        let extra = region.map(|r| vec![(h::CLIENT_REGION_NAME, r.to_string())]).unwrap_or_default();

        let reply = self
            .send_with(
                CALL,
                ServiceClient::DevicePairingProxy,
                reqwest::Method::POST,
                env.name(),
                url,
                DcgAuth::Msa(msa),
                trace,
                extra,
                Some(body),
            )
            .await?;

        Ok(reply.body)
    }

    /// `GET /DevicePairingProxy/PairingSessions/{id}?Api-Version=3.0.0` → the 2xx reply's body.
    pub async fn pairing_session(
        &self,
        env: Environment,
        msa: &str,
        session_id: &str,
        trace: &TraceContext,
    ) -> Result<String, RestError> {
        self.pairing_session_call("GetPairingSession", reqwest::Method::GET, env, msa, session_id, "", trace).await
    }

    /// `POST /DevicePairingProxy/PairingSessions/{id}/Complete?Api-Version=3.0.0`, no body → the 2xx
    /// reply's body.
    pub async fn complete_pairing_session(
        &self,
        env: Environment,
        msa: &str,
        session_id: &str,
        trace: &TraceContext,
    ) -> Result<String, RestError> {
        self.pairing_session_call(
            "CompletePairingSession",
            reqwest::Method::POST,
            env,
            msa,
            session_id,
            "/Complete",
            trace,
        )
        .await
    }

    /// `POST /DevicePairingProxy/PairingSessions/{id}/RefreshChallengePIN?Api-Version=3.0.0`, no body →
    /// the 2xx reply's body.
    pub async fn refresh_challenge_pin(
        &self,
        env: Environment,
        msa: &str,
        session_id: &str,
        trace: &TraceContext,
    ) -> Result<String, RestError> {
        self.pairing_session_call(
            "RefreshChallengePIN",
            reqwest::Method::POST,
            env,
            msa,
            session_id,
            "/RefreshChallengePIN",
            trace,
        )
        .await
    }

    /// `DevicePairingProxy/PairingSessions[/<id>]<suffix>?Api-Version=3.0.0` (the only service whose
    /// version parameter is spelled with capitals).
    pub(super) fn pairing_path(session_id: &str, suffix: &str) -> String {
        let id = if session_id.is_empty() { String::new() } else { format!("/{}", query_value(session_id)) };
        format!("DevicePairingProxy/PairingSessions{id}{suffix}?Api-Version={}", api_version::DEVICE_PAIRING_PROXY)
    }

    #[expect(clippy::too_many_arguments, reason = "independent inputs of one operation; not yet grouped")]
    pub(super) async fn pairing_session_call(
        &self,
        call: &'static str,
        method: reqwest::Method,
        env: Environment,
        msa: &str,
        session_id: &str,
        suffix: &str,
        trace: &TraceContext,
    ) -> Result<String, RestError> {
        if session_id.is_empty() {
            return Err(RestError::Local { call, detail: "empty pairing session id".into() });
        }

        let url = Self::url(self.base(env), &Self::pairing_path(session_id, suffix), call)?;
        let reply = self
            .send(call, ServiceClient::DevicePairingProxy, method, env.name(), url, DcgAuth::Msa(msa), trace, None)
            .await?;
        Ok(reply.body)
    }
}
