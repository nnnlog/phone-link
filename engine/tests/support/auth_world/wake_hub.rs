//! A scripted [`WakeHub`]: the flush result, the `OnConnected` region, and when the partner
//! appears. Waits return at once (recording the requested timeout) unless `Block` is chosen.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use phonelink_engine::seams::{HubRegionTimeout, WakeHub};
use phonelink_engine::types::{DcgClient, Environment, HubSendResult};
use tokio::sync::Notify;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Partner {
    /// Never appears: every wait reports "not connected".
    Never,
    /// Appears on the n-th partner wait (1-based) and every later one.
    OnWait(usize),
    /// Every wait blocks until `release` is notified, then reports connected.
    Block,
}

pub struct FakeWakeHub {
    pub flush_result: Mutex<HubSendResult>,
    pub region: Mutex<Result<Option<String>, HubRegionTimeout>>,
    pub partner: Mutex<Partner>,
    pub flushes: Mutex<Vec<DcgClient>>,
    pub region_waits: Mutex<Vec<(Environment, Duration)>>,
    pub partner_waits: Mutex<Vec<Duration>>,
    pub release: Notify,
    pub wait_started: Notify,
}

impl FakeWakeHub {
    pub fn new(partner: Partner) -> Arc<Self> {
        Arc::new(Self {
            flush_result: Mutex::new(HubSendResult::Success),
            region: Mutex::new(Ok(Some("westus2".into()))),
            partner: Mutex::new(partner),
            flushes: Mutex::new(Vec::new()),
            region_waits: Mutex::new(Vec::new()),
            partner_waits: Mutex::new(Vec::new()),
            release: Notify::new(),
            wait_started: Notify::new(),
        })
    }

    pub fn set_partner(&self, partner: Partner) {
        *self.partner.lock().unwrap() = partner;
        self.partner_waits.lock().unwrap().clear();
    }

    pub fn waits(&self) -> Vec<Duration> {
        self.partner_waits.lock().unwrap().clone()
    }

    pub fn flush_count(&self) -> usize {
        self.flushes.lock().unwrap().len()
    }
}

#[async_trait]
impl WakeHub for FakeWakeHub {
    async fn flush_connected(&self, target: &DcgClient) -> HubSendResult {
        self.flushes.lock().unwrap().push(target.clone());
        *self.flush_result.lock().unwrap()
    }

    async fn hub_region(&self, env: Environment, timeout: Duration) -> Result<Option<String>, HubRegionTimeout> {
        self.region_waits.lock().unwrap().push((env, timeout));
        self.region.lock().unwrap().clone()
    }

    async fn wait_partner_connected(&self, _target: &DcgClient, timeout: Duration) -> bool {
        let n = {
            let mut waits = self.partner_waits.lock().unwrap();
            waits.push(timeout);
            waits.len()
        };

        self.wait_started.notify_one();
        let partner = *self.partner.lock().unwrap();

        match partner {
            Partner::Never => false,
            Partner::OnWait(k) => n >= k,
            Partner::Block => {
                self.release.notified().await;
                true
            }
        }
    }
}
