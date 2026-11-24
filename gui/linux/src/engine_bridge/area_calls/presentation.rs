//! Which calls the incoming-call window and the call window show, when the ring starts and stops, and
//! which calls taken on the phone the user is told about.

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::time::Duration;

use phonelink_engine::api::calling::{ActiveCall, ActiveCallRole, ActiveCallStatus};

/// An incoming call's window shows for at most this long; the phone keeps ringing after it.
pub(super) const POPUP_LIFETIME: Duration = Duration::from_secs(60);
/// The notice of a call taken on the phone shows for at most this long.
pub(super) const NOTICE_LIFETIME: Duration = Duration::from_secs(7);

/// Which calls the incoming-call window and the call window show.
///
/// The incoming-call window shows every ringing call the user has not closed, for at most
/// [`POPUP_LIFETIME`]. The call window shows while a call placed or answered here, or transferred to
/// this computer, goes on. A call placed or answered on the phone gets a notice, once, when this
/// computer could take it and shows no call window.
#[derive(Default)]
pub(super) struct Presentation {
    /// Ringing calls the user closed, or whose lifetime ran out.
    pub(super) hidden: HashSet<u64>,
    /// The ringing calls whose lifetime started, each with its timer's token.
    lifetimes: HashMap<u64, u64>,
    next_token: u64,
    /// The incoming-call window shows (and rings).
    popup_shown: bool,
    /// The main call the call window shows.
    pub(super) window_call: Option<u64>,
    /// The calls taken on the phone that were seen, each with whether its notice shows.
    notices: HashMap<u64, bool>,
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
    /// Calls taken on the phone whose notice shows now.
    pub(super) notices: Vec<u64>,
    /// Calls whose notice is taken back.
    pub(super) withdrawn: Vec<u64>,
}

/// A call placed or answered on the phone, heard there, that this computer could take.
fn taken_on_phone(call: &ActiveCall) -> bool {
    call.role == ActiveCallRole::Untracked
        && matches!(call.status, ActiveCallStatus::Dialing | ActiveCallStatus::Talking)
        && !call.on_computer
        && call.can_transfer
}

impl Presentation {
    /// The ringing calls the incoming-call window shows.
    pub(super) fn popup<'a>(&'a self, calls: &'a [ActiveCall]) -> impl Iterator<Item = &'a ActiveCall> {
        calls.iter().filter(|call| call.status == ActiveCallStatus::Incoming && !self.hidden.contains(&call.id))
    }

    /// Follows the phone's current `calls`.
    ///
    /// Forgets the ones that ended, starts the lifetime of each newly shown ringing call, and says
    /// whether the ring starts or stops. A call newly taken on the phone gets its notice while
    /// `notices_allowed` (the user wants them, and no dial from here is under way); one seen while
    /// they are not allowed gets none later. A call that starts ringing while the desktop is not to be
    /// disturbed, which `undisturbed` reads when one does, is never shown: the phone rings alone.
    pub(super) fn update(
        &mut self,
        calls: &[ActiveCall],
        notices_allowed: bool,
        undisturbed: &mut dyn FnMut() -> bool,
    ) -> PresentationChange {
        let ids: HashSet<u64> = calls.iter().map(|call| call.id).collect();
        self.hidden.retain(|id| ids.contains(id));
        self.lifetimes.retain(|id, _| ids.contains(id));

        let newly_ringing: Vec<u64> =
            self.popup(calls).map(|call| call.id).filter(|id| !self.lifetimes.contains_key(id)).collect();
        if !newly_ringing.is_empty() && undisturbed() {
            self.hidden.extend(newly_ringing);
        }

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
            change.window = true;
        }

        for (id, shows) in &mut self.notices {
            if *shows && !calls.iter().any(|call| call.id == *id && taken_on_phone(call)) {
                *shows = false;
                change.withdrawn.push(*id);
            }
        }
        self.notices.retain(|id, _| ids.contains(id));
        for call in calls.iter().filter(|call| taken_on_phone(call)) {
            if let Entry::Vacant(seen) = self.notices.entry(call.id) {
                let shows = notices_allowed && primary.is_none();
                seen.insert(shows);
                if shows {
                    change.notices.push(call.id);
                }
            }
        }
        change
    }

    /// The notice of call `id` ran out or was answered: `true` when it showed until now.
    pub(super) fn notice_over(&mut self, id: u64) -> bool {
        self.notices.get_mut(&id).is_some_and(std::mem::take)
    }

    /// The lifetime `token` of call `id` ran out: its window closes.
    ///
    /// `false` when the call ended or the token is not its current one.
    pub(super) fn expire(&mut self, id: u64, token: u64) -> bool {
        self.lifetimes.get(&id) == Some(&token) && self.hidden.insert(id)
    }

    /// Call-control failures show in the call window while it is open.
    pub(super) fn window_shows_failures(&self) -> bool {
        self.window_call.is_some()
    }
}
