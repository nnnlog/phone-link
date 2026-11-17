//! Which calls the incoming-call window and the call window show, and when the ring starts and stops.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use phonelink_engine::api::calling::{ActiveCall, ActiveCallRole, ActiveCallStatus};

/// An incoming call's window shows for at most this long; the phone keeps ringing after it.
pub(super) const POPUP_LIFETIME: Duration = Duration::from_secs(60);

/// Which calls the incoming-call window and the call window show.
///
/// The incoming-call window shows every ringing call the user has not closed, for at most
/// [`POPUP_LIFETIME`]. The call window shows while a call placed or answered here goes on, opened
/// again for each new main call even after the user closed it.
#[derive(Default)]
pub(super) struct Presentation {
    /// Ringing calls the user closed, or whose lifetime ran out.
    pub(super) hidden: HashSet<u64>,
    /// The ringing calls whose lifetime started, each with its timer's token.
    lifetimes: HashMap<u64, u64>,
    next_token: u64,
    /// The incoming-call window shows (and rings).
    popup_shown: bool,
    /// The main call the call window was last opened for.
    pub(super) window_call: Option<u64>,
    /// The call window shows: the user has not closed it since its main call arrived.
    pub(super) window_open: bool,
}

/// What an update of the presentation asks for.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct PresentationChange {
    /// Ringing calls whose lifetime starts now, each with its timer's token.
    pub(super) lifetimes: Vec<(u64, u64)>,
    /// The ring starts (`Some(true)`) or stops (`Some(false)`).
    pub(super) ring: Option<bool>,
    /// The call window's main call changed: the window opens or closes.
    pub(super) window: bool,
}

impl Presentation {
    /// The ringing calls the incoming-call window shows.
    pub(super) fn popup<'a>(&'a self, calls: &'a [ActiveCall]) -> impl Iterator<Item = &'a ActiveCall> {
        calls.iter().filter(|call| call.status == ActiveCallStatus::Incoming && !self.hidden.contains(&call.id))
    }

    /// Follows the phone's current `calls`.
    ///
    /// Forgets the ones that ended, starts the lifetime of each newly shown ringing call, and says
    /// whether the ring starts or stops.
    pub(super) fn update(&mut self, calls: &[ActiveCall]) -> PresentationChange {
        let ids: HashSet<u64> = calls.iter().map(|call| call.id).collect();
        self.hidden.retain(|id| ids.contains(id));
        self.lifetimes.retain(|id, _| ids.contains(id));

        let popup: Vec<u64> = self.popup(calls).map(|call| call.id).collect();
        let mut change = PresentationChange::default();
        for id in &popup {
            if !self.lifetimes.contains_key(id) {
                let token = self.next_token;
                self.next_token += 1;
                self.lifetimes.insert(*id, token);
                change.lifetimes.push((*id, token));
            }
        }
        let shown = !popup.is_empty();
        if shown != self.popup_shown {
            self.popup_shown = shown;
            change.ring = Some(shown);
        }

        let primary = calls.iter().find(|call| call.role == ActiveCallRole::Primary).map(|call| call.id);
        if primary != self.window_call {
            self.window_call = primary;
            self.window_open = primary.is_some();
            change.window = true;
        }
        change
    }

    /// The lifetime `token` of call `id` ran out: its window closes.
    ///
    /// `false` when the call ended or the token is not its current one.
    pub(super) fn expire(&mut self, id: u64, token: u64) -> bool {
        self.lifetimes.get(&id) == Some(&token) && self.hidden.insert(id)
    }

    /// Call-control failures show in the call window while it is open.
    pub(super) fn window_shows_failures(&self) -> bool {
        self.window_open && self.window_call.is_some()
    }
}
