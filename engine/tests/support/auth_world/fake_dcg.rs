//! A fake DCG REST service on `127.0.0.1`: records every request and, unless a scripted reply is
//! queued for the operation, behaves like the real service — nonces are single-use, nonce JWTs are
//! verified (header, ES384 signature with the carried certificate, `iss` = CN), the identity
//! certificate must match on `SignIn`/`RotateKeys`, `Dcg-Token` must be a token it issued. Requests
//! under `beta/` and `df/` are the Beta / Dogfood bases. A test can observe an operation while the
//! client waits for it (`on_request`) and lose an operation's reply after the service applied it
//! (`lose_reply`).

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use phonelink_engine::auth::crypto::KeyPair;
use phonelink_engine::edge::Clock;
use phonelink_engine::edge::testing::ManualClock;
use phonelink_engine::types::{DeviceCertificates, DeviceInfoItem, new_guid_lower};
use serde_json::{Value, json};

use super::jwt;

/// One request as received.
#[derive(Debug, Clone)]
pub struct Recorded {
    /// `Prod`, `Beta` or `Dogfood` (from the base prefix).
    pub env: &'static str,
    pub method: String,
    /// Path without the base prefix and the leading `/`, e.g. `Auth/SignIn`.
    pub path: String,
    /// `/<path>?<query>`.
    pub path_and_query: String,
    /// Header names lower-cased, in arrival order.
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Recorded {
    pub fn header(&self, name: &str) -> Option<&str> {
        let name = name.to_ascii_lowercase();
        let found: Vec<&str> = self.headers.iter().filter(|(k, _)| *k == name).map(|(_, v)| v.as_str()).collect();
        assert!(found.len() <= 1, "header {name} sent {} times", found.len());
        found.first().copied()
    }

    pub fn json(&self) -> Value {
        serde_json::from_str(&self.body)
            .unwrap_or_else(|e| panic!("{} body is not JSON ({e}): {}", self.path, self.body))
    }

    /// The last path segment (`SignIn`).
    pub fn op(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or("")
    }

    /// `METHOD /path?query`.
    pub fn line(&self) -> String {
        format!("{} {}", self.method, self.path_and_query)
    }

    pub fn query(&self, key: &str) -> Option<String> {
        let q = self.path_and_query.split_once('?')?.1;
        q.split('&').find_map(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            (k == key).then(|| v.to_string())
        })
    }
}

/// A queued reply: status, body, and a `Retry-After` value.
pub type Scripted = (u16, String, Option<String>);

/// Observes one request of an operation as it arrives.
pub type RequestHook = Box<dyn FnMut(&Recorded) + Send>;

pub struct FakeDcgState {
    pub clock: Arc<ManualClock>,
    pub requests: Vec<Recorded>,
    /// Queued replies per operation (last path segment), served before the default behaviour.
    pub scripted: HashMap<String, VecDeque<Scripted>>,
    /// `serviceEnvironment` of discovery (`None` omits the field).
    pub service_environment: Option<String>,
    pub nonce_counter: u64,
    pub nonces: HashSet<String>,
    /// Registered identity certificates by lower-case device id.
    pub identities: HashMap<String, Vec<u8>>,
    /// Issued services tokens → device id.
    pub tokens: HashMap<String, String>,
    pub token_counter: u64,
    pub token_lifetime: Duration,
    pub key_valid_remaining_days: Option<i64>,
    /// `deviceId` echoed by `SignIn` instead of the caller's.
    pub sign_in_device_id_override: Option<String>,
    pub account_id: String,
    pub account_cert: Vec<u8>,
    /// Enrolled PCs by lower-case id (their device-list entries).
    pub enrolled: HashMap<String, DeviceInfoItem>,
    /// Other devices of the account.
    pub peers: Vec<DeviceInfoItem>,
    /// A raw `GetDeviceInfoList` body served instead of the computed list.
    pub device_list_override: Option<String>,
    pub rotations: u32,
    /// Called with each request of the operation (last path segment) when it arrives, while the
    /// client waits for the reply.
    pub on_request: HashMap<String, RequestHook>,
    /// How many next replies of an operation are lost: the service applies the request, then the
    /// connection breaks before the reply body.
    pub lose_reply: HashMap<String, u32>,
}

impl FakeDcgState {
    fn now_s(&self) -> i64 {
        self.clock.now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64
    }
}

type Shared = Arc<Mutex<FakeDcgState>>;

pub struct FakeDcg {
    /// `http://127.0.0.1:<port>/` — the prod base.
    pub base: String,
    pub state: Shared,
}

pub fn error_body(category: Option<i64>, code: &str) -> String {
    match category {
        Some(c) => {
            json!({"error": {"categoryCode": c, "code": code, "message": "scripted", "details": []}}).to_string()
        }
        None => json!({"error": {"code": code, "message": "scripted", "details": []}}).to_string(),
    }
}

impl FakeDcg {
    pub async fn start(clock: Arc<ManualClock>) -> Self {
        let account_id = new_guid_lower();
        let account_cert = KeyPair::generate(&account_id, clock.now()).unwrap().cert.der;
        let state = Arc::new(Mutex::new(FakeDcgState {
            clock,
            requests: Vec::new(),
            scripted: HashMap::new(),
            service_environment: Some("Production".into()),
            nonce_counter: 0,
            nonces: HashSet::new(),
            identities: HashMap::new(),
            tokens: HashMap::new(),
            token_counter: 0,
            token_lifetime: Duration::from_secs(24 * 3600),
            key_valid_remaining_days: Some(365),
            sign_in_device_id_override: None,
            account_id,
            account_cert,
            enrolled: HashMap::new(),
            peers: Vec::new(),
            device_list_override: None,
            rotations: 0,
            on_request: HashMap::new(),
            lose_reply: HashMap::new(),
        }));

        let app = Router::new().fallback(handle).with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self { base: format!("http://{addr}/"), state }
    }

    pub fn with<R>(&self, f: impl FnOnce(&mut FakeDcgState) -> R) -> R {
        f(&mut self.state.lock().unwrap())
    }

    pub fn requests(&self) -> Vec<Recorded> {
        self.with(|s| s.requests.clone())
    }

    pub fn take_requests(&self) -> Vec<Recorded> {
        self.with(|s| std::mem::take(&mut s.requests))
    }

    /// `METHOD /path?query` of every request so far.
    pub fn lines(&self) -> Vec<String> {
        self.requests().iter().map(Recorded::line).collect()
    }

    /// Requests of one operation.
    pub fn of(&self, op: &str) -> Vec<Recorded> {
        self.requests().into_iter().filter(|r| r.op() == op).collect()
    }

    pub fn count(&self, op: &str) -> usize {
        self.of(op).len()
    }

    /// Queues `status`/`body` as the next reply to `op`.
    pub fn script(&self, op: &str, status: u16, body: &str) {
        self.with(|s| s.scripted.entry(op.to_string()).or_default().push_back((status, body.to_string(), None)));
    }

    /// Queues `status` with an empty body and `Retry-After: retry_after` as the next reply to `op`.
    pub fn script_retry_after(&self, op: &str, status: u16, retry_after: &str) {
        self.with(|s| {
            s.scripted.entry(op.to_string()).or_default().push_back((status, String::new(), Some(retry_after.into())))
        });
    }

    /// Queues `status`/`body` as the next `times` replies to `op`.
    pub fn script_times(&self, op: &str, times: usize, status: u16, body: &str) {
        for _ in 0..times {
            self.script(op, status, body);
        }
    }

    /// Calls `f` with every request of `op` as it arrives (the client is waiting for the reply).
    pub fn on_request(&self, op: &str, f: impl FnMut(&Recorded) + Send + 'static) {
        self.with(|s| s.on_request.insert(op.to_string(), Box::new(f)));
    }

    /// The next reply to `op` is lost after the service applied the request.
    pub fn lose_next_reply(&self, op: &str) {
        self.with(|s| *s.lose_reply.entry(op.to_string()).or_default() += 1);
    }

    pub fn account_id(&self) -> String {
        self.with(|s| s.account_id.clone())
    }

    pub fn account_cert(&self) -> Vec<u8> {
        self.with(|s| s.account_cert.clone())
    }

    pub fn add_peer(&self, item: DeviceInfoItem) {
        self.with(|s| s.peers.push(item));
    }

    pub fn peer_mut<R>(&self, id: &str, f: impl FnOnce(&mut DeviceInfoItem) -> R) -> R {
        self.with(|s| f(s.peers.iter_mut().find(|p| p.id == id).expect("peer")))
    }

    pub fn own_entry_mut<R>(&self, id: &str, f: impl FnOnce(&mut Option<DeviceInfoItem>) -> R) -> R {
        self.with(|s| {
            let mut entry = s.enrolled.remove(&id.to_ascii_lowercase());
            let r = f(&mut entry);
            if let Some(e) = entry {
                s.enrolled.insert(id.to_ascii_lowercase(), e);
            }
            r
        })
    }
}

fn reply(status: u16, body: String) -> Response {
    let mut resp = (StatusCode::from_u16(status).unwrap(), body).into_response();
    resp.headers_mut().insert("content-type", "application/json".parse().unwrap());
    resp
}

fn ok_json(v: Value) -> Response {
    reply(200, v.to_string())
}

fn ok_empty() -> Response {
    reply(200, String::new())
}

fn bad(code: &str, message: impl std::fmt::Display) -> Response {
    reply(400, json!({"error": {"categoryCode": 2, "code": code, "message": message.to_string()}}).to_string())
}

async fn handle(State(state): State<Shared>, method: Method, uri: Uri, headers: HeaderMap, body: Bytes) -> Response {
    let raw_path = uri.path().trim_start_matches('/').to_string();
    let (env, path) = if let Some(rest) = raw_path.strip_prefix("beta/") {
        ("Beta", rest.to_string())
    } else if let Some(rest) = raw_path.strip_prefix("df/") {
        ("Dogfood", rest.to_string())
    } else {
        ("Prod", raw_path.clone())
    };

    let rec = Recorded {
        env,
        method: method.to_string(),
        path: path.clone(),
        path_and_query: format!("/{path}{}", uri.query().map(|q| format!("?{q}")).unwrap_or_default()),
        headers: headers
            .iter()
            .map(|(k, v)| (k.as_str().to_ascii_lowercase(), v.to_str().unwrap_or("<non-ascii>").to_string()))
            .collect(),
        body: String::from_utf8(body.to_vec()).unwrap(),
    };

    let mut st = state.lock().unwrap();
    st.requests.push(rec.clone());
    let op = rec.op().to_string();
    if let Some(f) = st.on_request.get_mut(&op) {
        f(&rec);
    }

    if let Some((status, body, retry_after)) = st.scripted.get_mut(&op).and_then(VecDeque::pop_front) {
        let mut resp = reply(status, body);
        if let Some(v) = retry_after {
            resp.headers_mut().insert("retry-after", v.parse().unwrap());
        }
        return resp;
    }

    let response = serve(&mut st, &rec, &path);
    match st.lose_reply.get_mut(&op) {
        Some(n) if *n > 0 => {
            *n -= 1;
            lost_reply()
        }
        _ => response,
    }
}

/// A reply whose body breaks off: the client gets no usable reply (a transport failure).
fn lost_reply() -> Response {
    let body = futures_util::stream::once(async { Err::<Bytes, std::io::Error>(std::io::Error::other("reply lost")) });
    Response::builder().status(StatusCode::OK).body(axum::body::Body::from_stream(body)).unwrap()
}

fn serve(st: &mut FakeDcgState, rec: &Recorded, path: &str) -> Response {
    match (rec.method.as_str(), path) {
        ("GET", "EnvironmentDiscovery/UserServiceEnvironment") => match &st.service_environment {
            Some(e) => ok_json(json!({ "serviceEnvironment": e })),
            None => ok_json(json!({})),
        },
        ("POST", "Auth/GenerateNonce") => generate_nonce(st, rec),
        ("POST", "Auth/CreateIdentity") => create_identity(st, rec),
        ("POST", "Auth/SignIn") => sign_in(st, rec),
        ("POST", "Auth/RotateKeys") => rotate_keys(st, rec),
        ("POST", "DeviceAuthProxy/EnrollDevice") => enroll(st, rec, false),
        ("POST", "DeviceAuthProxy/SetDeviceInfo") => enroll(st, rec, true),
        ("GET", "DeviceAuthProxy/GetDeviceInfoList") => device_list(st),
        ("GET", "DeviceAuthProxy/GetAccountCert") => ok_json(json!({ "accountCert": jwt::b64(&st.account_cert) })),
        ("POST", "DeviceAuthProxy/UnlinkDevice") => unlink(st, rec),
        ("DELETE", "DeviceAuthProxy/DeleteDeviceInfo") => delete_own(st, rec),
        ("POST", "Dispatcher/Wake") => ok_empty(),
        _ => reply(404, String::new()),
    }
}

fn generate_nonce(st: &mut FakeDcgState, rec: &Recorded) -> Response {
    let body = rec.json();
    if body["deviceId"].as_str().is_none_or(str::is_empty) {
        return bad("InvalidParameters", "no deviceId");
    }

    st.nonce_counter += 1;
    let nonce = format!("nonce-{}", st.nonce_counter);
    st.nonces.insert(nonce.clone());
    ok_json(json!({ "nonce": nonce, "epochTimeStamp": st.now_s() }))
}

/// Verifies a nonce JWT; returns `(iss, nonce, certificate DER)`. `consume` removes the nonce.
fn verify_nonce_jwt(st: &mut FakeDcgState, token: &str, consume: bool) -> Result<(String, String, Vec<u8>), String> {
    let jwt = jwt::decode(token);
    if jwt.header != r#"{"alg":"ES384","typ":"JWT"}"# {
        return Err(format!("header {}", jwt.header));
    }
    let names = jwt.claim_order();
    if names != ["iss", "Nonce", "Certificate", "nbf", "exp"] {
        return Err(format!("claims {names:?}"));
    }

    let der = jwt::unb64(jwt.str("Certificate"));
    if !jwt::verifies(&jwt, &der) {
        return Err("signature".into());
    }
    let iss = jwt.str("iss").to_string();
    if jwt::cert_cn(&der) != iss {
        return Err("iss is not the certificate CN".into());
    }

    let now = st.now_s();
    if !(jwt.i64("nbf") < now && now < jwt.i64("exp")) {
        return Err("lifetime".into());
    }
    let nonce = jwt.str("Nonce").to_string();
    if consume && !st.nonces.remove(&nonce) {
        return Err(format!("unknown nonce {nonce}"));
    }

    Ok((iss, nonce, der))
}

fn token_response(st: &mut FakeDcgState, device_id: &str, echo: &str) -> Response {
    st.token_counter += 1;
    let token = format!("svc-token-{}", st.token_counter);
    st.tokens.insert(token.clone(), device_id.to_ascii_lowercase());

    let mut v = json!({
        "accessToken": token,
        "deviceId": echo,
        "epochExpirationTime": st.now_s() + st.token_lifetime.as_secs() as i64,
        "tenantId": "fake-tenant",
    });
    if let Some(days) = st.key_valid_remaining_days {
        v["keyValidRemainingDays"] = json!(days);
    }
    ok_json(v)
}

fn create_identity(st: &mut FakeDcgState, rec: &Recorded) -> Response {
    let body = rec.json();
    let Some(token) = body["certificateJWT"].as_str() else { return bad("InvalidRequest", "no certificateJWT") };
    match verify_nonce_jwt(st, token, true) {
        Ok((iss, _, der)) => {
            st.identities.insert(iss.to_ascii_lowercase(), der);
            token_response(st, &iss, &iss)
        }
        Err(e) => bad("InvalidJWT", e),
    }
}

fn sign_in(st: &mut FakeDcgState, rec: &Recorded) -> Response {
    let body = rec.json();
    let Some(token) = body["certificateJWT"].as_str() else { return bad("InvalidRequest", "no certificateJWT") };
    match verify_nonce_jwt(st, token, true) {
        Ok((iss, _, der)) => {
            if st.identities.get(&iss.to_ascii_lowercase()) != Some(&der) {
                return bad("InvalidCertificate", "not the registered identity certificate");
            }
            let echo = st.sign_in_device_id_override.clone().unwrap_or_else(|| iss.clone());
            token_response(st, &iss, &echo)
        }
        Err(e) => bad("InvalidJWT", e),
    }
}

fn rotate_keys(st: &mut FakeDcgState, rec: &Recorded) -> Response {
    let body = rec.json();
    let (Some(old), Some(new)) = (body["oldCertificateJWT"].as_str(), body["newCertificateJWT"].as_str()) else {
        return bad("InvalidRequest", "missing JWTs");
    };

    let (old_iss, old_nonce, old_der) = match verify_nonce_jwt(st, old, true) {
        Ok(v) => v,
        Err(e) => return bad("InvalidJWT", e),
    };
    if st.identities.get(&old_iss.to_ascii_lowercase()) != Some(&old_der) {
        return bad("InvalidCertificate", "old certificate is not the registered one");
    }

    let (new_iss, new_nonce, new_der) = match verify_nonce_jwt(st, new, false) {
        Ok(v) => v,
        Err(e) => return bad("InvalidJWT", e),
    };
    if new_iss != old_iss || new_nonce != old_nonce || new_der == old_der {
        return bad("InvalidJWT", "new JWT must sign the same nonce with a new certificate of the same subject");
    }

    st.identities.insert(old_iss.to_ascii_lowercase(), new_der);
    st.rotations += 1;
    ok_empty()
}

fn enroll(st: &mut FakeDcgState, rec: &Recorded, set_info: bool) -> Response {
    let Some(device_id) = rec.header("dcg-token").and_then(|t| st.tokens.get(t)).cloned() else {
        return reply(401, json!({"error": {"categoryCode": 3, "code": "InvalidAuthHeader"}}).to_string());
    };
    if rec.header("authorization").is_none_or(|a| !a.starts_with("Bearer ")) {
        return reply(401, json!({"error": {"categoryCode": 1, "code": "InvalidAuthHeader"}}).to_string());
    }

    let body = rec.json();
    let m = &body["metadata"];
    let Some(cert) = body["certificates"]["SelfSigned"][0].as_str() else {
        return bad("InvalidParameters", "no SelfSigned certificate");
    };

    let s = |v: &Value| v.as_str().map(str::to_string);
    let entry = DeviceInfoItem {
        id: device_id.clone(),
        is_linked: Some(true),
        certificates: Some(DeviceCertificates {
            ct_self_signed: None,
            self_signed: Some(vec![cert.to_string()]),
            pki: None,
        }),
        capabilities: m["capabilities"].as_array().map(|a| a.iter().filter_map(&s).collect()),
        client_type: s(&m["clientType"]),
        client_version: s(&m["clientVersion"]),
        consent_version: m["consentVersion"].as_i64(),
        custom_data: s(&m["customData"]),
        is_enabled: m["isEnabled"].as_bool(),
        last_seen_time: Some(st.now_s()),
        manufacture: s(&m["manufacture"]),
        model_name: s(&m["modelName"]),
        model_version: s(&m["modelVersion"]),
        name: s(&m["displayName"]),
        os_name: s(&m["osName"]),
        os_version: s(&m["osVersion"]),
        registration_time: Some(st.now_s()),
        ..Default::default()
    };

    st.enrolled.insert(device_id.clone(), entry.clone());
    if set_info {
        return ok_empty();
    }
    ok_json(json!({
        "accountCert": jwt::b64(&st.account_cert),
        "accountInfo": {"accountKey": "k", "firstName": "F", "lastName": "L", "signInName": "user@example.com"},
        // Not consumed by the PC: even an unknown certificates key must not break enrollment.
        "devices": [entry, {"id": "other", "certificates": {"Weird": []}}],
        "rootCertificateChain": ["AA=="],
    }))
}

fn device_list(st: &FakeDcgState) -> Response {
    if let Some(raw) = &st.device_list_override {
        return reply(200, raw.clone());
    }
    let mut items: Vec<&DeviceInfoItem> = st.enrolled.values().collect();
    items.extend(st.peers.iter());
    ok_json(serde_json::to_value(items).unwrap())
}

fn delete_own(st: &mut FakeDcgState, rec: &Recorded) -> Response {
    if rec.query("device-id").as_deref() != Some("") {
        return bad("InvalidParameters", "device-id must be empty");
    }
    let Some(own) = rec.header("dcg-token").and_then(|t| st.tokens.get(t).cloned()) else {
        return reply(401, json!({"error": {"categoryCode": 3}}).to_string());
    };
    st.enrolled.remove(&own);
    ok_empty()
}

fn unlink(st: &mut FakeDcgState, rec: &Recorded) -> Response {
    // As the service: a body-less POST without a length is refused.
    if rec.header("content-length") != Some("0") {
        return reply(411, String::new());
    }
    let Some(id) = rec.query("device-id") else { return bad("InvalidParameters", "no device-id") };
    if rec.header("dcg-token").is_none_or(|t| !st.tokens.contains_key(t)) {
        return reply(401, json!({"error": {"categoryCode": 3}}).to_string());
    }
    for p in st.peers.iter_mut().filter(|p| p.id.eq_ignore_ascii_case(&id)) {
        p.is_linked = Some(false);
    }
    ok_empty()
}

/// `SystemTime` of a Unix second (helper for assertions).
pub fn at(secs: i64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(secs as u64)
}
