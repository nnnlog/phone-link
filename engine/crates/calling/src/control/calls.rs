//! The phone's calls as the Mac knows them: built from the AG's call list (`+CLCC`), with the calls
//! placed or answered from the Mac tracked as primary and secondary.

use super::at::ClccEntry;

/// A call's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallStatus {
    /// Ringing: an incoming call, or a waiting one while another call exists.
    Incoming,
    /// An outgoing call being dialed or ringing at the other end.
    Dialing,
    Talking,
    Held,
}

impl CallStatus {
    /// `+CLCC` `<stat>`: 0 active, 1 held, 2 dialing, 3 alerting, 4 incoming, 5 waiting, 6 held by
    /// response and hold; another value is no call status.
    fn of(stat: u8) -> Option<CallStatus> {
        match stat {
            0 => Some(CallStatus::Talking),
            1 | 6 => Some(CallStatus::Held),
            2 | 3 => Some(CallStatus::Dialing),
            4 | 5 => Some(CallStatus::Incoming),
            _ => None,
        }
    }
}

/// Where a call shows on the Mac.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallRole {
    /// Placed or answered on the phone: only an incoming call shows (its popup).
    Untracked,
    /// The call window's main call.
    Primary,
    /// The call window's other call.
    Secondary,
}

/// One call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    /// The Mac's id for the call, never reused.
    pub id: u64,
    /// The AG's index (`<idx>`).
    pub index: u32,
    pub incoming: bool,
    pub status: CallStatus,
    /// Incoming while another call exists (`<stat>` 5).
    pub waiting: bool,
    /// The other party's number; empty when the phone gave none.
    pub number: String,
    /// When the Mac first saw the call talking, Unix ms.
    pub talking_since_ms: Option<i64>,
    pub role: CallRole,
}

/// What the tracked calls' rules ask the controller to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FollowUp {
    /// End this call: a third tracked call pushes the old secondary out.
    End(u64),
    /// Resume this held call: the other tracked call ended.
    Resume(u64),
}

/// The calls and the roles of the tracked ones.
#[derive(Debug, Default)]
pub(crate) struct CallBook {
    calls: Vec<Call>,
    next_id: u64,
    /// The last number `+CLIP` or `+CCWA` gave, for an incoming call the list gives none.
    caller: Option<String>,
    primary: Option<u64>,
    secondary: Option<u64>,
    /// A dial from the Mac was answered `OK`: the first outgoing call with an id above this one is
    /// tracked.
    expecting_dialed: Option<u64>,
}

impl CallBook {
    pub(crate) fn calls(&self) -> Vec<Call> {
        self.calls.clone()
    }

    pub(crate) fn get(&self, id: u64) -> Option<&Call> {
        self.calls.iter().find(|c| c.id == id)
    }

    pub(crate) fn set_caller(&mut self, number: Option<String>) {
        if number.is_some() {
            self.caller = number;
        }
    }

    pub(crate) fn expect_dialed(&mut self) {
        self.expecting_dialed = Some(self.next_id);
    }

    /// Every call ended with the channel.
    pub(crate) fn clear(&mut self) {
        self.calls.clear();
        self.caller = None;
        self.primary = None;
        self.secondary = None;
        self.expecting_dialed = None;
    }

    /// Replaces the calls with the AG's list. A call stays the same while its index stays with the
    /// same direction; one whose index left the list has ended. Returns what the tracked calls' rules
    /// ask for.
    pub(crate) fn apply(&mut self, entries: &[ClccEntry], now_ms: i64) -> Vec<FollowUp> {
        let mut follow = Vec::new();
        let mut kept = Vec::with_capacity(entries.len());

        for entry in entries {
            let Some(status) = CallStatus::of(entry.status) else { continue };
            let existing = self.calls.iter().position(|c| c.index == entry.index && c.incoming == entry.incoming);
            let mut call = match existing {
                Some(i) => self.calls.swap_remove(i),
                None => self.new_call(entry, status),
            };

            call.status = status;
            call.waiting = entry.status == 5;
            if let Some(number) = &entry.number {
                call.number.clone_from(number);
            }
            if status == CallStatus::Talking && call.talking_since_ms.is_none() {
                call.talking_since_ms = Some(now_ms);
            }
            kept.push(call);
        }

        let ended: Vec<u64> = self.calls.iter().map(|c| c.id).collect();
        self.calls = kept;
        self.calls.sort_by_key(|c| c.index);
        for id in ended {
            follow.extend(self.ended(id));
        }

        let dialed = self
            .expecting_dialed
            .and_then(|after| self.calls.iter().find(|c| !c.incoming && c.id > after))
            .map(|c| c.id);
        if let Some(id) = dialed {
            self.expecting_dialed = None;
            follow.extend(self.track(id));
        }

        if !self.calls.iter().any(|c| c.status == CallStatus::Incoming) {
            self.caller = None;
        }
        follow
    }

    fn new_call(&mut self, entry: &ClccEntry, status: CallStatus) -> Call {
        self.next_id += 1;
        let number = if entry.incoming { self.caller.clone() } else { None };
        Call {
            id: self.next_id,
            index: entry.index,
            incoming: entry.incoming,
            status,
            waiting: false,
            number: number.unwrap_or_default(),
            talking_since_ms: None,
            role: CallRole::Untracked,
        }
    }

    /// Makes `id` a tracked call: the first is primary; a new one ends the secondary if there is one,
    /// demotes the primary to secondary and becomes primary.
    pub(crate) fn track(&mut self, id: u64) -> Option<FollowUp> {
        if self.get(id).is_none() || self.primary == Some(id) || self.secondary == Some(id) {
            return None;
        }

        let pushed_out = match self.primary {
            None => None,
            Some(primary) => {
                let old = self.secondary.replace(primary);
                old.map(FollowUp::End)
            }
        };
        self.primary = Some(id);
        self.sync_roles();
        pushed_out
    }

    /// Exchanges the tracked calls' roles (a swap).
    pub(crate) fn swap_roles(&mut self) {
        if self.secondary.is_some() {
            std::mem::swap(&mut self.primary, &mut self.secondary);
            self.sync_roles();
        }
    }

    /// `id` ended: when it was tracked, the other tracked call becomes primary and is resumed when
    /// held.
    fn ended(&mut self, id: u64) -> Option<FollowUp> {
        if self.primary == Some(id) {
            self.primary = self.secondary.take();
        } else if self.secondary == Some(id) {
            self.secondary = None;
        } else {
            return None;
        }

        self.sync_roles();
        let remaining = self.primary.and_then(|p| self.get(p))?;
        (remaining.status == CallStatus::Held).then_some(FollowUp::Resume(remaining.id))
    }

    fn sync_roles(&mut self) {
        let (primary, secondary) = (self.primary, self.secondary);
        for call in &mut self.calls {
            call.role = if Some(call.id) == primary {
                CallRole::Primary
            } else if Some(call.id) == secondary {
                CallRole::Secondary
            } else {
                CallRole::Untracked
            };
        }
    }
}

#[cfg(test)]
mod tests;
