//! The device calls: enrolling, device info, the device list and account certificate, unlinking, deleting, waking.

use super::{
    AccountCertResponse, DcgAuth, DcgRest, DeviceInfoBody, EnrollDeviceResponse, RawEnrollResponse, RestError,
    WakeBody, query_value, sanitize_pop_device_key,
};
use crate::auth::rest_policy::ServiceClient;
use phonelink_core::constants::api_version;
use phonelink_core::types::{DeviceInfoItem, Environment, TraceContext};

impl DcgRest {
    /// `POST /DeviceAuthProxy/EnrollDevice?api-version=1.5.0&pop-device-key=<key>` with the
    /// enrollment's MSA token and `Dcg-Token`. `pop_device_key` is sanitized (blank
    /// unless it has the pairing-token shape).
    pub async fn enroll_device(
        &self,
        env: Environment,
        msa: &str,
        services: &str,
        pop_device_key: &str,
        body: &DeviceInfoBody,
        trace: &TraceContext,
    ) -> Result<EnrollDeviceResponse, RestError> {
        const CALL: &str = "EnrollDevice";
        let url = Self::url(
            self.base(env),
            &format!(
                "DeviceAuthProxy/EnrollDevice?api-version={}&pop-device-key={}",
                api_version::DEVICE_AUTH_PROXY_DEVICES,
                query_value(sanitize_pop_device_key(pop_device_key))
            ),
            CALL,
        )?;
        let body = Self::json(CALL, body)?;

        let reply = self
            .send(
                CALL,
                ServiceClient::DeviceManagement,
                reqwest::Method::POST,
                env.name(),
                url,
                DcgAuth::MsaWithDcgToken { msa, services },
                trace,
                Some(body),
            )
            .await?;

        let raw: RawEnrollResponse = Self::parse(CALL, &reply)?;
        Ok(EnrollDeviceResponse { account_cert: raw.account_cert.and_then(|v| v.as_str().map(str::to_string)) })
    }

    /// `POST /DeviceAuthProxy/SetDeviceInfo?api-version=1.5.0&pop-device-key=` (empty) → empty.
    pub async fn set_device_info(
        &self,
        env: Environment,
        msa: &str,
        services: &str,
        body: &DeviceInfoBody,
        trace: &TraceContext,
    ) -> Result<(), RestError> {
        const CALL: &str = "SetDeviceInfo";
        let url = Self::url(
            self.base(env),
            &format!(
                "DeviceAuthProxy/SetDeviceInfo?api-version={}&pop-device-key=",
                api_version::DEVICE_AUTH_PROXY_DEVICES
            ),
            CALL,
        )?;
        let body = Self::json(CALL, body)?;

        self.send(
            CALL,
            ServiceClient::DeviceManagement,
            reqwest::Method::POST,
            env.name(),
            url,
            DcgAuth::MsaWithDcgToken { msa, services },
            trace,
            Some(body),
        )
        .await
        .map(|_| ())
    }

    /// `GET /DeviceAuthProxy/GetDeviceInfoList?api-version=1.5.0&filter=&top=20` → `DeviceInfoItem[]`.
    /// A `certificates` object with any key but `CTSelfSigned` / `SelfSigned` / `PKI` makes the
    /// whole list invalid.
    pub async fn device_info_list(
        &self,
        env: Environment,
        msa: &str,
        trace: &TraceContext,
    ) -> Result<Vec<DeviceInfoItem>, RestError> {
        const CALL: &str = "GetDeviceInfoList";
        let url = Self::url(
            self.base(env),
            &format!(
                "DeviceAuthProxy/GetDeviceInfoList?api-version={}&filter=&top=20",
                api_version::DEVICE_AUTH_PROXY_DEVICES
            ),
            CALL,
        )?;

        let reply = self
            .send(
                CALL,
                ServiceClient::DeviceManagement,
                reqwest::Method::GET,
                env.name(),
                url,
                DcgAuth::Msa(msa),
                trace,
                None,
            )
            .await?;

        Self::parse(CALL, &reply)
    }

    /// `GET /DeviceAuthProxy/GetAccountCert?api-version=1.0.0` → `accountCert` (`None` when absent
    /// or not a string).
    pub async fn account_cert(
        &self,
        env: Environment,
        msa: &str,
        trace: &TraceContext,
    ) -> Result<Option<String>, RestError> {
        const CALL: &str = "GetAccountCert";
        let url = Self::url(
            self.base(env),
            &format!("DeviceAuthProxy/GetAccountCert?api-version={}", api_version::DEVICE_AUTH_PROXY_TRUST),
            CALL,
        )?;

        let reply = self
            .send(
                CALL,
                ServiceClient::DeviceAuthProxy,
                reqwest::Method::GET,
                env.name(),
                url,
                DcgAuth::Msa(msa),
                trace,
                None,
            )
            .await?;

        let parsed: AccountCertResponse = Self::parse(CALL, &reply)?;
        Ok(parsed.account_cert.and_then(|v| v.as_str().map(str::to_string)))
    }

    /// `POST /DeviceAuthProxy/UnlinkDevice?api-version=1.5.0&device-id=<peer>`, no body → empty.
    pub async fn unlink_device(
        &self,
        env: Environment,
        msa: &str,
        services: &str,
        peer_id: &str,
        trace: &TraceContext,
    ) -> Result<(), RestError> {
        const CALL: &str = "UnlinkDevice";
        if peer_id.is_empty() {
            return Err(RestError::Local { call: CALL, detail: "empty device-id".into() });
        }

        let url = Self::url(
            self.base(env),
            &format!(
                "DeviceAuthProxy/UnlinkDevice?api-version={}&device-id={}",
                api_version::DEVICE_AUTH_PROXY_DEVICES,
                query_value(peer_id)
            ),
            CALL,
        )?;

        self.send(
            CALL,
            ServiceClient::DeviceManagement,
            reqwest::Method::POST,
            env.name(),
            url,
            DcgAuth::MsaWithDcgToken { msa, services },
            trace,
            None,
        )
        .await
        .map(|_| ())
    }

    /// `DELETE /DeviceAuthProxy/DeleteDeviceInfo?api-version=1.5.0&device-id=` (empty: this device's
    /// own record), no body → empty.
    pub async fn delete_device_info(
        &self,
        env: Environment,
        msa: &str,
        services: &str,
        trace: &TraceContext,
    ) -> Result<(), RestError> {
        const CALL: &str = "DeleteDeviceInfo";
        let url = Self::url(
            self.base(env),
            &format!(
                "DeviceAuthProxy/DeleteDeviceInfo?api-version={}&device-id=",
                api_version::DEVICE_AUTH_PROXY_DEVICES
            ),
            CALL,
        )?;

        self.send(
            CALL,
            ServiceClient::DeviceManagement,
            reqwest::Method::DELETE,
            env.name(),
            url,
            DcgAuth::MsaWithDcgToken { msa, services },
            trace,
            None,
        )
        .await
        .map(|_| ())
    }

    /// `POST /Dispatcher/Wake?api-version=1.5.0` `WakeBody` with the services-token bearer → empty.
    pub async fn wake(
        &self,
        env: Environment,
        services: &str,
        body: &WakeBody,
        trace: &TraceContext,
    ) -> Result<(), RestError> {
        const CALL: &str = "Wake";
        let url =
            Self::url(self.base(env), &format!("Dispatcher/Wake?api-version={}", api_version::DISPATCHER_WAKE), CALL)?;
        let body = Self::json(CALL, body)?;

        self.send(
            CALL,
            ServiceClient::Dispatcher,
            reqwest::Method::POST,
            env.name(),
            url,
            DcgAuth::Services(services),
            trace,
            Some(body),
        )
        .await
        .map(|_| ())
    }
}
