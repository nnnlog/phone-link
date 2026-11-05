//! The nearby-device prompt and its token.

use super::{Inner, NearbyFlow, NearbySignal, Out, PhoneArgs, Scoped, SetupFailure, State, Transaction, merged};
use crate::errors::code;
use crate::machine::{PhoneCommand, PhoneState};
use crate::wire::{Body, LockedState, scope, workflow};
use crate::{MirroringError, NearbyPrompt, wire};
use phonelink_core::sync::MutexExt;
use phonelink_nano::channel::message::{InboundMessage, MessageChannel};
use std::sync::Arc;
use tracing::{info, warn};

impl Inner {
    pub(in super::super) fn nearby_send(&self, target: &str, body: Vec<u8>) -> Result<(), MirroringError> {
        let c = self.state.lock_or_panic().nearby_channel.clone();
        let c = c.ok_or(MirroringError::NotNow("the nearby check is not running"))?;
        Ok(c.send(target, &body)?)
    }

    pub(super) fn nearby_prompt(
        &self,
        st: &mut State,
        skip_unlock: bool,
        skip_verification: bool,
        wrong: bool,
        out: &mut Out,
    ) {
        let prompt = NearbyPrompt {
            device_id: self.env.app.selected_phone().map(|p| p.id),
            expiry_token: st.nearby_flow.expiry_token,
            token_expired: st.nearby_flow.expired,
            pin_code_wrong: wrong,
            skip_unlock,
            skip_verification,
            china_market: self
                .env
                .app
                .phone_record()
                .and_then(|r| r.market)
                .is_some_and(|m| m.eq_ignore_ascii_case("CN")),
        };

        let args = PhoneArgs { nearby: true, ..Default::default() };
        if self.apply_phone(st, PhoneCommand::NearbyConnectionNeeded, args, out) {
            st.nearby = Some(prompt);
        }
    }

    pub(super) fn start_token_timer(&self) {
        let epoch = {
            let mut st = self.state.lock_or_panic();
            st.nearby_flow.timer_epoch += 1;
            st.nearby_flow.timer_epoch
        };

        let me = self.weak.clone();
        let after = self.config.pin_code_expiry;
        self.spawn(async move {
            tokio::time::sleep(after).await;
            let Some(me) = me.upgrade() else { return };

            let mut out = Out::default();
            {
                let mut st = me.state.lock_or_panic();
                if st.nearby_flow.timer_epoch != epoch {
                    return;
                }

                st.nearby_flow.expiry_token = rand::random_range(10_000..100_000);
                st.nearby_flow.expired = true;
                if st.phone == PhoneState::NearbyConnectionNeeded {
                    me.nearby_prompt(&mut st, false, false, false, &mut out);
                }
            }

            me.flush(out);
        });
    }

    pub(in super::super) fn nearby_code_refreshed(&self) {
        self.state.lock_or_panic().nearby_flow.expired = false;
        self.start_token_timer();
    }

    pub(super) async fn nearby_check(self: Arc<Self>) -> Result<(), SetupFailure> {
        self.nearby_signal.send_replace(NearbySignal::default());
        let existing = {
            let mut st = self.state.lock_or_panic();
            st.nearby_flow.waiting = false;
            st.nearby_channel.clone()
        };
        let (channel, local) = match existing {
            Some(c) => (c, self.state.lock_or_panic().nearby_flow.local),
            None => self.open_nearby_channel().await?,
        };

        let (tx, rx) = self.begin_transaction(Transaction::Other);
        channel.send(scope::NEARBY_REQUEST_SETTING, &wire::nearby_request_setting(local, &tx)).map_err(|error| {
            SetupFailure::NotSent { code: code::MESSAGE_CHANNEL_FAILED, route: scope::NEARBY_REQUEST_SETTING, error }
        })?;

        let mut failed = self.nearby_signal.subscribe();
        let body = tokio::select! {
            r = rx => r.map_err(|_| SetupFailure::Error(code::MESSAGE_CHANNEL_FAILED))?,
            s = failed.wait_for(|s| s.failed.is_some()) => {
                let code = s.ok().and_then(|s| s.failed).unwrap_or(code::DISCONNECTED_UNKNOWN);
                return Err(SetupFailure::Error(code));
            }
        };

        let b = Body::parse(&body).map_err(|error| SetupFailure::Malformed {
            code: code::DISCONNECTED_UNKNOWN,
            route: scope::NEARBY_REQUEST_SETTING_RESULT,
            error,
        })?;

        let locked = b.opt_bool("isLocked").unwrap_or(false) || b.opt_bool("isSecureLocked").unwrap_or(false);
        let skip_unlock = b.opt_bool("skipUnlock").unwrap_or(false);
        let skip_verification = b.opt_bool("skipVerification").unwrap_or(false);
        info!(is_local = local, locked, skip_unlock, skip_verification, "nearby setting received");

        let settings = self.settings();
        let wait_unlock = local && !skip_unlock && !settings.do_not_ask_skip_unlock;
        let wait_verify = !local && !skip_verification;
        if wait_unlock || wait_verify {
            self.wait_for_nearby_user(wait_unlock, wait_verify, locked).await?;
        }

        self.phone_command(PhoneCommand::NearbyConnectionCompleted, PhoneArgs::default());
        Ok(())
    }

    /// Opens the nearby workflow's channel and starts taking its messages: the channel, and whether
    /// the phone is on the local network.
    async fn open_nearby_channel(self: &Arc<Self>) -> Result<(Arc<MessageChannel>, bool), SetupFailure> {
        let audio = self.values().is_ok_and(|v| v.audio());
        let start = self.links.start(workflow::NEARBY, wire::NEARBY_INBOUND, true, audio);
        let started = match tokio::time::timeout(self.config.channel_open, start).await {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => return Err(e.into()),
            Err(_) => {
                self.links.stop(workflow::NEARBY);
                return Err(SetupFailure::Error(code::MESSAGE_CHANNEL_FAILED));
            }
        };

        let c = started.channel.clone();
        {
            let mut st = self.state.lock_or_panic();
            st.nearby_channel = Some(c.clone());
            st.nearby_flow = NearbyFlow {
                expiry_token: rand::random_range(10_000..100_000),
                local: started.local,
                ..Default::default()
            };
        }

        let inbound = merged(started.inbound, c.closed());
        let me = self.clone();
        let cc = c.clone();
        self.spawn(async move {
            let mut inbound = inbound;
            while let Some(Scoped { m, .. }) = inbound.recv().await {
                me.nearby_message(&m);
            }

            // Only the current channel's end fails the check: an earlier connection's channel
            // ending (its idle connection replaced) must not fail the check on the new one.
            let mut st = me.state.lock_or_panic();
            if st.nearby_channel.as_ref().is_some_and(|n| Arc::ptr_eq(n, &cc)) {
                st.nearby_channel = None;
                drop(st);
                me.nearby_signal.send_modify(|s| s.failed = Some(code::DISCONNECTED_UNKNOWN));
            }
        });
        Ok((c, started.local))
    }

    /// Shows the prompt the nearby check waits on (the locked state when the phone is `locked`) and
    /// waits for the phone to be unlocked (`wait_unlock`) or verified (`wait_verify`).
    async fn wait_for_nearby_user(
        &self,
        wait_unlock: bool,
        wait_verify: bool,
        locked: bool,
    ) -> Result<(), SetupFailure> {
        if wait_verify {
            self.start_token_timer();
        }

        let mut out = Out::default();
        {
            let mut st = self.state.lock_or_panic();
            st.nearby_flow.waiting = true;
            if locked {
                let l = LockedState { is_virtual_device_streaming_supported: true, ..Default::default() };
                self.apply_phone(&mut st, PhoneCommand::OnSecureLocked, PhoneArgs::lock(l), &mut out);
            } else {
                self.nearby_prompt(&mut st, wait_unlock, false, false, &mut out);
            }
        }
        self.flush(out);

        let mut rx = self.nearby_signal.subscribe();
        let signal = rx
            .wait_for(|s| s.failed.is_some() || (wait_unlock && s.unlock) || (wait_verify && s.proceed))
            .await
            .map(|s| *s)
            .map_err(|_| SetupFailure::Error(code::DISCONNECTED_UNKNOWN))?;
        if let Some(code) = signal.failed {
            return Err(SetupFailure::Error(code));
        }
        Ok(())
    }

    pub(super) fn nearby_message(self: &Arc<Self>, m: &InboundMessage) {
        let body = Body::parse(&m.body).ok();
        match m.target.as_str() {
            scope::NEARBY_REQUEST_SETTING_RESULT => {
                let tx = body.as_ref().and_then(|b| b.opt_str("transactionId"));
                self.complete_transaction(tx.as_deref(), &m.body);
            }
            scope::NEARBY_VERIFICATION_RESULT => self.on_verification_result(body),
            scope::NEARBY_WRONG_PIN_CODE => self.on_wrong_pin_code(),
            scope::NEARBY_SKIP_UNLOCK_RESULT => self.nearby_signal.send_modify(|s| s.unlock = true),
            scope::NEARBY_SKIP_VERIFICATION_RESULT => self.nearby_signal.send_modify(|s| s.proceed = true),
            scope::NEARBY_DEVICE_LOCK => self.on_device_lock(body),
            other => warn!(raw_target = other, "nearby message without a handler; dropped"),
        }
    }
    /// `verificationResult`: a matching token verifies the phone.
    fn on_verification_result(&self, body: Option<Body>) {
        let Some(b) = body else { return warn!("malformed verificationResult; dropped") };
        let is_qr = b.opt_bool("isQrCode").unwrap_or(false);
        let token = b.opt_str("expiryToken").and_then(|t| t.parse::<i32>().ok());

        let mut out = Out::default();
        {
            let mut st = self.state.lock_or_panic();
            if !is_qr || token.is_none() || token == Some(st.nearby_flow.expiry_token) {
                st.nearby_flow.verified = true;
                st.nearby_flow.expiry_token = 0;
                st.nearby_flow.expired = false;
                st.nearby_flow.timer_epoch += 1;
                if self.settings().do_not_ask_skip_verification {
                    self.nearby_signal.send_modify(|s| s.proceed = true);
                } else if st.phone == PhoneState::NearbyConnectionNeeded {
                    self.nearby_prompt(&mut st, false, true, false, &mut out);
                }
            }
        }

        self.flush(out);
    }

    /// `wrongPinCode`: the prompt shows the code was wrong.
    fn on_wrong_pin_code(&self) {
        let mut out = Out::default();
        {
            let mut st = self.state.lock_or_panic();
            if st.phone == PhoneState::NearbyConnectionNeeded {
                self.nearby_prompt(&mut st, false, false, true, &mut out);
            }
        }
        self.flush(out);
    }

    /// `deviceLock`: the phone's lock changed while the check waits on the user.
    fn on_device_lock(&self, body: Option<Body>) {
        let Some(b) = body else { return warn!("malformed deviceLock; dropped") };
        let locked = b.opt_bool("isLocked").unwrap_or(false) || b.opt_bool("isSecureLocked").unwrap_or(false);

        let mut out = Out::default();
        {
            let mut st = self.state.lock_or_panic();
            let local = st.nearby_flow.local;
            let signal = *self.nearby_signal.borrow();
            let waiting = st.nearby_flow.waiting && if local { !signal.unlock } else { !signal.proceed };

            if waiting && signal.failed.is_none() {
                if locked {
                    let l = LockedState { is_virtual_device_streaming_supported: true, ..Default::default() };
                    self.apply_phone(&mut st, PhoneCommand::OnSecureLocked, PhoneArgs::lock(l), &mut out);
                } else if local && self.settings().do_not_ask_skip_unlock {
                    self.nearby_signal.send_modify(|s| s.unlock = true);
                } else {
                    let verified = st.nearby_flow.verified;
                    self.nearby_prompt(&mut st, local, !local && verified, false, &mut out);
                }
            }
        }

        self.flush(out);
    }
}
