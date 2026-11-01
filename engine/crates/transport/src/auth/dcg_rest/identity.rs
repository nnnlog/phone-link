//! The identity calls: the service environment, nonces, creating and signing in an identity, rotating its keys.

use super::{
    CertificateJwtBody, DcgAuth, DcgRest, DeviceIdBody, EnvironmentResponse, NonceResponse, RawTokenResponse, Reply,
    RestError, RotateKeysBody, TokenResponse,
};
use crate::auth::rest_policy::ServiceClient;
use phonelink_core::constants::api_version;
use phonelink_core::types::{Environment, TraceContext};

impl DcgRest {
    /// `GET /EnvironmentDiscovery/UserServiceEnvironment?api-version=1.0.0` on the discovery
    /// (prod) base with the prod-scope MSA token. Returns `serviceEnvironment` when it is a string.
    pub async fn user_service_environment(
        &self,
        msa_prod: &str,
        trace: &TraceContext,
    ) -> Result<Option<String>, RestError> {
        const CALL: &str = "UserServiceEnvironment";
        let url = Self::url(
            &self.endpoints.discovery_base,
            &format!("EnvironmentDiscovery/UserServiceEnvironment?api-version={}", api_version::ENVIRONMENT_DISCOVERY),
            CALL,
        )?;

        let reply = self
            .send(
                CALL,
                ServiceClient::EnvironmentDiscovery,
                reqwest::Method::GET,
                "discovery",
                url,
                DcgAuth::Msa(msa_prod),
                trace,
                None,
            )
            .await?;

        let parsed: EnvironmentResponse = Self::parse(CALL, &reply)?;
        Ok(parsed.service_environment.and_then(|v| v.as_str().map(str::to_string)))
    }

    /// `POST /Auth/GenerateNonce?api-version=1.1.0` `{ deviceId }` → the nonce (a missing nonce is
    /// a hard failure).
    pub async fn generate_nonce(
        &self,
        env: Environment,
        msa: &str,
        device_id: &str,
        trace: &TraceContext,
    ) -> Result<String, RestError> {
        const CALL: &str = "GenerateNonce";
        let url = Self::url(self.base(env), &format!("Auth/GenerateNonce?api-version={}", api_version::AUTH), CALL)?;
        let body = Self::json(CALL, &DeviceIdBody { device_id })?;

        let reply = self
            .send(
                CALL,
                ServiceClient::Auth,
                reqwest::Method::POST,
                env.name(),
                url,
                DcgAuth::MsaUserIdentity(msa),
                trace,
                Some(body),
            )
            .await?;

        let parsed: NonceResponse = Self::parse(CALL, &reply)?;
        match parsed.nonce {
            Some(n) if !n.is_empty() => Ok(n),
            _ => Err(RestError::InvalidResponse { call: CALL, detail: "no nonce".into() }),
        }
    }

    pub(super) fn token_response(call: &'static str, reply: &Reply) -> Result<TokenResponse, RestError> {
        let raw: RawTokenResponse = Self::parse(call, reply)?;

        let access_token = match raw.access_token {
            Some(t) if !t.is_empty() => t,
            _ => return Err(RestError::InvalidResponse { call, detail: "empty or missing accessToken".into() }),
        };
        let device_id = raw
            .device_id
            .filter(|d| !d.is_empty())
            .ok_or_else(|| RestError::InvalidResponse { call, detail: "missing deviceId".into() })?;
        let epoch_expiration_time = raw
            .epoch_expiration_time
            .ok_or_else(|| RestError::InvalidResponse { call, detail: "missing epochExpirationTime".into() })?;

        Ok(TokenResponse {
            access_token,
            device_id,
            epoch_expiration_time,
            key_valid_remaining_days: raw.key_valid_remaining_days,
        })
    }

    pub(super) async fn certificate_jwt_call(
        &self,
        call: &'static str,
        path: &str,
        env: Environment,
        msa: &str,
        jwt: &str,
        trace: &TraceContext,
    ) -> Result<TokenResponse, RestError> {
        let url = Self::url(self.base(env), &format!("{path}?api-version={}", api_version::AUTH), call)?;
        let body = Self::json(call, &CertificateJwtBody { certificate_jwt: jwt })?;

        let reply = self
            .send(
                call,
                ServiceClient::Auth,
                reqwest::Method::POST,
                env.name(),
                url,
                DcgAuth::MsaUserIdentity(msa),
                trace,
                Some(body),
            )
            .await?;

        Self::token_response(call, &reply)
    }

    /// `POST /Auth/CreateIdentity?api-version=1.1.0` `{ certificateJWT }` → `TokenResponse`.
    pub async fn create_identity(
        &self,
        env: Environment,
        msa: &str,
        certificate_jwt: &str,
        trace: &TraceContext,
    ) -> Result<TokenResponse, RestError> {
        self.certificate_jwt_call("CreateIdentity", "Auth/CreateIdentity", env, msa, certificate_jwt, trace).await
    }

    /// `POST /Auth/SignIn?api-version=1.1.0` `{ certificateJWT }` → `TokenResponse`.
    pub async fn sign_in(
        &self,
        env: Environment,
        msa: &str,
        certificate_jwt: &str,
        trace: &TraceContext,
    ) -> Result<TokenResponse, RestError> {
        self.certificate_jwt_call("SignIn", "Auth/SignIn", env, msa, certificate_jwt, trace).await
    }

    /// `POST /Auth/RotateKeys?api-version=1.1.0` `{ oldCertificateJWT, newCertificateJWT }` → empty.
    pub async fn rotate_keys(
        &self,
        env: Environment,
        msa: &str,
        old_jwt: &str,
        new_jwt: &str,
        trace: &TraceContext,
    ) -> Result<(), RestError> {
        const CALL: &str = "RotateKeys";
        let url = Self::url(self.base(env), &format!("Auth/RotateKeys?api-version={}", api_version::AUTH), CALL)?;
        let body = Self::json(CALL, &RotateKeysBody { old_certificate_jwt: old_jwt, new_certificate_jwt: new_jwt })?;

        self.send(
            CALL,
            ServiceClient::Auth,
            reqwest::Method::POST,
            env.name(),
            url,
            DcgAuth::MsaUserIdentity(msa),
            trace,
            Some(body),
        )
        .await
        .map(|_| ())
    }
}
