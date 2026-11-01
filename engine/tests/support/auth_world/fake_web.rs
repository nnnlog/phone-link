//! A fake sign-in web view: it records every page the broker asks it to host and answers as the user
//! would. A sign-in page posts a `DAProperty` for [`FakeWebState::puid`], the account the user signs
//! in with, with a numbered inline flow token (`ft-1`, `ft-2`, …); a consent page reaches its end
//! page without a new sign-in result. A queued outcome replaces the next answer.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use phonelink_engine::edge::{WebSignIn, WebSignInError, WebSignInKind, WebSignInRequest};

/// The account PUID the fake sign-in reports unless a test picks another.
pub const SEED_PUID: &str = "0003BFFD0SEEDPUID";

/// The sign-in result a page posts for `puid`, spending `inline_ft` on the next exchange.
pub fn da_property_json_with_ft(puid: &str, inline_ft: &str) -> String {
    format!(
        r#"{{"sDAToken":"<EncryptedData xmlns=\"x\"></EncryptedData>","sSTSInlineFlowToken":"{inline_ft}","sSigninName":"seed@example.com","sDAExpires":"2099-01-01T00:00:00Z","K":"{puid}"}}"#
    )
}

/// The sign-in result a page posts for `puid`.
pub fn da_property_json(puid: &str) -> String {
    da_property_json_with_ft(puid, "ft")
}

/// What a consent page that finishes without signing anyone in posts: no `DAProperty`.
const CONSENT_DONE: &str = "{}";

pub struct FakeWebState {
    pub requests: Vec<WebSignInRequest>,
    /// The account the user signs in with on the next sign-in pages.
    pub puid: String,
    /// Answered to the next pages, in order, before the default answers.
    pub queued: VecDeque<Result<String, WebSignInError>>,
    /// Sign-in results posted so far (numbers the inline flow tokens).
    pub posted: u64,
}

pub struct FakeWeb {
    state: Mutex<FakeWebState>,
}

impl FakeWeb {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(FakeWebState {
                requests: Vec::new(),
                puid: SEED_PUID.to_string(),
                queued: VecDeque::new(),
                posted: 0,
            }),
        })
    }

    pub fn with<R>(&self, f: impl FnOnce(&mut FakeWebState) -> R) -> R {
        f(&mut self.state.lock().unwrap())
    }

    /// Queues `outcome` for the next page.
    pub fn queue(&self, outcome: Result<String, WebSignInError>) {
        self.with(|s| s.queued.push_back(outcome));
    }

    /// The kind of every page hosted so far.
    pub fn kinds(&self) -> Vec<WebSignInKind> {
        self.with(|s| s.requests.iter().map(|r| r.kind).collect())
    }

    /// The account the user signs in with from now on.
    pub fn sign_in_as(&self, puid: &str) {
        self.with(|s| s.puid = puid.to_string());
    }
}

#[async_trait::async_trait]
impl WebSignIn for FakeWeb {
    async fn present(&self, request: WebSignInRequest) -> Result<String, WebSignInError> {
        let mut st = self.state.lock().unwrap();
        let kind = request.kind;
        st.requests.push(request);

        if let Some(outcome) = st.queued.pop_front() {
            return outcome;
        }

        match kind {
            WebSignInKind::Login => {
                st.posted += 1;
                Ok(da_property_json_with_ft(&st.puid, &format!("ft-{}", st.posted)))
            }
            WebSignInKind::Consent => Ok(CONSENT_DONE.to_string()),
        }
    }
}
