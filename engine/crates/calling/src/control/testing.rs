//! A scripted phone for call-control tests: its Bluetooth radio and paired device, and an HFP Audio
//! Gateway on one RFCOMM channel that answers the Mac's AT commands from a call list the test drives.

use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use phonelink_bluetooth::{PairingError, PhoneBluetooth, RfcommError};
use phonelink_core::sync::MutexExt;

/// The paired phone's address.
pub const ADDRESS: &str = "aa-bb-cc-dd-ee-ff";

/// The indicators in the order the phone of the recorded run listed them.
const INDICATORS: [&str; 7] = ["call", "callsetup", "service", "signal", "roam", "battchg", "callheld"];

/// One of the AG's calls, as `+CLCC` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgCall {
    pub index: u32,
    pub incoming: bool,
    /// 0 active, 1 held, 2 dialing, 3 alerting, 4 incoming, 5 waiting.
    pub status: u8,
    pub number: String,
}

#[derive(Default)]
struct State {
    radio: bool,
    paired: Option<String>,
    open_failures: VecDeque<RfcommError>,
    opened: Vec<String>,
    channel_open: bool,
    /// The phone closed the channel.
    dropped: bool,
    outbox: Vec<u8>,
    commands: Vec<String>,
    calls: Vec<AgCall>,
    refused: HashSet<String>,
    silent: HashSet<String>,
    pending: String,
}

/// The scripted phone.
pub struct FakeAg {
    state: Mutex<State>,
    readable: Condvar,
}

impl FakeAg {
    /// Radio on, the phone paired, no calls.
    pub fn new() -> Arc<Self> {
        let state = State { radio: true, paired: Some(ADDRESS.into()), ..State::default() };
        Arc::new(Self { state: Mutex::new(state), readable: Condvar::new() })
    }

    pub fn set_radio(&self, on: bool) {
        self.state.lock_or_panic().radio = on;
    }

    pub fn set_paired(&self, paired: bool) {
        self.state.lock_or_panic().paired = paired.then(|| ADDRESS.to_string());
    }

    /// The next channel openings fail with these, in order.
    pub fn fail_opens(&self, failures: Vec<RfcommError>) {
        self.state.lock_or_panic().open_failures.extend(failures);
    }

    /// Commands starting with `prefix` are answered `ERROR`.
    pub fn refuse(&self, prefix: &str) {
        self.state.lock_or_panic().refused.insert(prefix.to_string());
    }

    /// Commands starting with `prefix` get no answer.
    pub fn ignore(&self, prefix: &str) {
        self.state.lock_or_panic().silent.insert(prefix.to_string());
    }

    /// Every command the Mac sent, in order.
    pub fn commands(&self) -> Vec<String> {
        self.state.lock_or_panic().commands.clone()
    }

    /// The service UUIDs the Mac opened channels to.
    pub fn opened(&self) -> Vec<String> {
        self.state.lock_or_panic().opened.clone()
    }

    pub fn calls(&self) -> Vec<AgCall> {
        self.state.lock_or_panic().calls.clone()
    }

    /// A call comes in from `number`: waiting when another call exists.
    pub fn ring(&self, number: &str) {
        let mut s = self.state.lock_or_panic();
        let status = if s.calls.is_empty() { 4 } else { 5 };
        let index = next_index(&s.calls);
        s.calls.push(AgCall { index, incoming: true, status, number: number.into() });
        if status == 4 {
            send(&mut s, &["+CIEV: 2,1", "RING", &format!("+CLIP: \"{number}\",145")]);
        } else {
            send(&mut s, &[&format!("+CCWA: \"{number}\",145,1"), "+CIEV: 2,1"]);
        }
        self.readable.notify_all();
    }

    /// The other party answered the outgoing call.
    pub fn remote_answers(&self) {
        let mut s = self.state.lock_or_panic();
        for c in s.calls.iter_mut().filter(|c| matches!(c.status, 2 | 3)) {
            c.status = 0;
        }
        send(&mut s, &["+CIEV: 1,1", "+CIEV: 2,0"]);
        self.readable.notify_all();
    }

    /// The other party of call `index` hung up.
    pub fn remote_hangs_up(&self, index: u32) {
        let mut s = self.state.lock_or_panic();
        s.calls.retain(|c| c.index != index);
        send(&mut s, &["+CIEV: 1,0"]);
        self.readable.notify_all();
    }

    /// The phone sends `line` unprompted.
    pub fn say(&self, line: &str) {
        let mut s = self.state.lock_or_panic();
        send(&mut s, &[line]);
        self.readable.notify_all();
    }

    /// The phone went out of range: the channel closes.
    pub fn drop_channel(&self) {
        self.state.lock_or_panic().dropped = true;
        self.readable.notify_all();
    }

    fn answer(&self, s: &mut State, command: &str) {
        s.commands.push(command.to_string());
        if s.silent.iter().any(|p| command.starts_with(p.as_str())) {
            return;
        }
        if s.refused.iter().any(|p| command.starts_with(p.as_str())) {
            send(s, &["ERROR"]);
            return;
        }

        match command {
            c if c.starts_with("AT+BRSF=") => send(s, &["+BRSF: 4079", "OK"]),
            "AT+CIND=?" => {
                let names: Vec<String> = INDICATORS.iter().map(|n| format!("(\"{n}\",(0,1))")).collect();
                send(s, &[&format!("+CIND: {}", names.join(",")), "OK"]);
            }
            "AT+CIND?" => send(s, &["+CIND: 0,0,1,4,0,3,0", "OK"]),
            "AT+CMER=3,0,0,1" | "AT+CLIP=1" | "AT+CCWA=1" => send(s, &["OK"]),
            "AT+CHLD=?" => send(s, &["+CHLD: (0,1,1x,2,2x,3,4)", "OK"]),
            "AT+CLCC" => {
                let lines: Vec<String> = s
                    .calls
                    .iter()
                    .map(|c| {
                        format!("+CLCC: {},{},{},0,0,\"{}\",129", c.index, u8::from(c.incoming), c.status, c.number)
                    })
                    .collect();
                let mut all: Vec<&str> = lines.iter().map(String::as_str).collect();
                all.push("OK");
                send(s, &all);
            }
            "ATA" => {
                for c in s.calls.iter_mut().filter(|c| c.status == 4) {
                    c.status = 0;
                }
                send(s, &["OK", "+CIEV: 1,1", "+CIEV: 2,0"]);
            }
            c if c.starts_with("ATD") => {
                let number = c.trim_start_matches("ATD").trim_end_matches(';').to_string();
                let index = next_index(&s.calls);
                s.calls.push(AgCall { index, incoming: false, status: 2, number });
                send(s, &["OK", "+CIEV: 2,2"]);
            }
            "AT+CHUP" => {
                let target = s
                    .calls
                    .iter()
                    .position(|c| c.status == 4)
                    .or_else(|| s.calls.iter().position(|c| matches!(c.status, 0 | 2 | 3)));
                if let Some(i) = target {
                    s.calls.remove(i);
                }
                send(s, &["OK", "+CIEV: 1,0"]);
            }
            "AT+CHLD=0" => {
                let waiting = s.calls.iter().any(|c| c.status == 5);
                s.calls.retain(|c| if waiting { c.status != 5 } else { c.status != 1 });
                send(s, &["OK", "+CIEV: 7,0"]);
            }
            "AT+CHLD=2" => {
                for c in &mut s.calls {
                    c.status = match c.status {
                        0 => 1,
                        1 | 5 => 0,
                        other => other,
                    };
                }
                send(s, &["OK", "+CIEV: 7,1"]);
            }
            _ => send(s, &["ERROR"]),
        }
    }
}

fn next_index(calls: &[AgCall]) -> u32 {
    (1..).find(|i| calls.iter().all(|c| c.index != *i)).expect("a free index")
}

fn send(s: &mut State, lines: &[&str]) {
    for line in lines {
        s.outbox.extend_from_slice(format!("\r\n{line}\r\n").as_bytes());
    }
}

impl PhoneBluetooth for FakeAg {
    fn radio_on(&self) -> bool {
        self.state.lock_or_panic().radio
    }
    fn paired_phone(&self, _names: Vec<String>) -> Option<String> {
        self.state.lock_or_panic().paired.clone()
    }
    fn discover_and_pair(&self, _names: Vec<String>, _timeout_ms: u64) -> Result<String, PairingError> {
        let mut s = self.state.lock_or_panic();
        s.paired = Some(ADDRESS.into());
        Ok(ADDRESS.into())
    }
    fn rfcomm_open(&self, _address: String, service_uuid: String, _timeout_ms: u64) -> Result<u64, RfcommError> {
        let mut s = self.state.lock_or_panic();
        s.opened.push(service_uuid);
        if let Some(e) = s.open_failures.pop_front() {
            return Err(e);
        }
        s.channel_open = true;
        s.dropped = false;
        s.outbox.clear();
        s.pending.clear();
        Ok(1)
    }
    fn refresh_services(&self, _address: String, _service_uuid: String, _timeout_ms: u64) -> Result<bool, RfcommError> {
        Ok(true)
    }
    fn rfcomm_write(&self, _channel: u64, bytes: Vec<u8>) -> Result<(), RfcommError> {
        let mut s = self.state.lock_or_panic();
        if !s.channel_open || s.dropped {
            return Err(RfcommError::Io { reason: "the channel is closed".into() });
        }

        s.pending.push_str(&String::from_utf8_lossy(&bytes));
        while let Some(end) = s.pending.find('\r') {
            let command: String = s.pending.drain(..=end).collect();
            let command = command.trim().to_string();
            if !command.is_empty() {
                self.answer(&mut s, &command);
            }
        }
        self.readable.notify_all();
        Ok(())
    }
    fn rfcomm_read(&self, _channel: u64, timeout_ms: u64) -> Result<Vec<u8>, RfcommError> {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        let mut s = self.state.lock_or_panic();
        loop {
            if !s.outbox.is_empty() {
                return Ok(std::mem::take(&mut s.outbox));
            }
            if !s.channel_open || s.dropped {
                return Ok(Vec::new());
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(RfcommError::TimedOut);
            }
            s = self.readable.wait_timeout(s, left).map(|(g, _)| g).unwrap_or_else(|e| e.into_inner().0);
        }
    }
    fn rfcomm_close(&self, _channel: u64) {
        self.state.lock_or_panic().channel_open = false;
        self.readable.notify_all();
    }
}
