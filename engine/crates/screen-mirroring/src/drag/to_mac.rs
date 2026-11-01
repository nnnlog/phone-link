//! Dragging from the phone to the Mac.

use super::{
    ACTION_CANCEL, DragWriteError, FromPhone, FromPhoneItem, IDLE, ItemStatus, NO_DROP_WAIT, Next, PROGRESS_INTERVAL,
    Phase, PhoneDragEvent, PhoneDragItem, State, WindowDrag, cancel_blobs, completion_text, mac_file_name, next,
    progress_title, reason, receive, remove_dir, size_text,
};
use crate::machine::WindowState;
use crate::store::FEATURE;
use crate::wire::ClipData;
use crate::wire::scope;
use crate::{ScreenMirroringEvent, wire};
use phonelink_core::sync::MutexExt;
use phonelink_core::types::new_guid_lower;
use phonelink_features::shared::notifier::{NotificationAction, SystemNotification};
use phonelink_nano::channel::ChannelError;
use phonelink_nano::channel::blob::BlobChannel;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

impl WindowDrag {
    /// Applies `event`: a reset at an end (`CancelBroadcast` first tells the phone).
    pub(super) fn step(&self, st: &mut State, event: PhoneDragEvent) -> Next {
        let n = next(st.phase, event);
        debug!(session = %self.session_id, from_phase = ?st.phase, ?event, next_phase = ?n, "phone drag step");

        match n {
            Next::To(p) => {
                if p == Phase::Drop && st.phase == Phase::DragStarted {
                    self.post_progress(st, true);
                }
                st.phase = p;
                let outside = matches!(p, Phase::WatchingPointer | Phase::RequestingOwnership | Phase::DragStarted);
                self.input.lock_or_panic().set_drag_from_phone(outside || matches!(p, Phase::Drop | Phase::CancelWait));
            }
            end => {
                if end == Next::CancelBroadcast {
                    let body = wire::drag_cancel(&new_guid_lower(), reason::USER_DID_NOT_DROP.into());
                    self.send(st, scope::DRAG_CANCEL, &body);
                }
                self.reset(st, event);
            }
        }
        n
    }

    /// The Mac drag of the phone's files ended without a write: "user cancelled", and again when
    /// that reached `CancelWait`.
    pub(super) fn drag_cancelled(&self, st: &mut State) {
        if matches!(st.phase, Phase::Ready) {
            return;
        }
        if let Next::To(Phase::CancelWait) = self.step(st, PhoneDragEvent::UserCancelled) {
            self.step(st, PhoneDragEvent::UserCancelled);
        }
    }

    /// The one line of a phone drag's end, with why it was given up when it was.
    fn log_drag_end(&self, event: PhoneDragEvent, drag: &FromPhone) {
        let (file_count, received_count) = (drag.items.len(), drag.received_files);
        match &drag.failure {
            Some(failure) => warn!(
                session = %self.session_id,
                ?event,
                file_count,
                received_count,
                failure = %failure,
                "phone drag given up"
            ),
            None => info!(session = %self.session_id, ?event, file_count, received_count, "phone drag ended"),
        }
    }

    pub(super) fn reset(&self, st: &mut State, event: PhoneDragEvent) {
        st.phase = Phase::Ready;
        st.pending = None;
        st.phone_epoch += 1;
        {
            let mut input = self.input.lock_or_panic();
            input.set_drag_from_phone(false);
            input.release_pointers();
        }

        if let Some(drag) = st.from_phone.take() {
            self.log_drag_end(event, &drag);
            drag.stop.cancel();

            if let Some((_, blob)) = &st.channels {
                let receiving = drag.items.iter().filter(|i| *i.status.borrow() == ItemStatus::Receiving);
                cancel_blobs(blob, receiving.map(|i| i.item.data_id.as_bytes()));
                for i in &drag.items {
                    blob.unregister(i.item.data_id.as_bytes());
                }
            }

            remove_dir(&drag.dir);
            if drag.released {
                // Ended by the user or by no destination: no outcome to tell.
                if matches!(event, PhoneDragEvent::UserCancelled | PhoneDragEvent::NoDropTarget) {
                    if let Err(e) = self.notifier.withdraw(vec![self.notification_id()]) {
                        warn!(error = %e, "drag notification not withdrawn");
                    }
                } else {
                    let (title, body) = completion_text(drag.items.len(), drag.received_files);
                    self.notify(title, body, false, Vec::new());
                }
            }
        }

        let _ = self.events.send(ScreenMirroringEvent::DragEnded { session_id: self.session_id.clone() });
    }

    /// A position of the pointer (mouse, touch or pen; a move or a press or release) normalized to the
    /// video. Checked only while the window has the input focus.
    pub fn pointer_at(&self, x: f64, y: f64) {
        if !self.input.lock_or_panic().focused() {
            return;
        }

        let mut st = self.state.lock_or_panic();
        if st.phase == Phase::WatchingPointer && !((0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y)) {
            if let Next::To(Phase::RequestingOwnership) = self.step(&mut st, PhoneDragEvent::OutOfBounds) {
                self.send(&st, scope::DRAG_REQUEST_OWNERSHIP, &wire::drag_request_ownership(&new_guid_lower()));
            }
        }
    }

    /// The window lost the input focus; `left_held` = the left mouse button's press had gone to the
    /// phone and was released by the loss.
    pub fn focus_lost(self: &Arc<Self>, left_held: bool) {
        let epoch = {
            let st = self.state.lock_or_panic();
            if !left_held || st.phase != Phase::DragStarted {
                return;
            }
            st.phone_epoch
        };

        let me = self.clone();
        self.runtime.spawn(async move {
            tokio::time::sleep(NO_DROP_WAIT).await;
            let mut st = me.state.lock_or_panic();
            if st.phone_epoch == epoch && st.phase == Phase::DragStarted {
                me.step(&mut st, PhoneDragEvent::NoDropTarget);
            }
        });
    }

    /// The window's state changed: a drag in progress ends outside Loading, Connected and Locked.
    pub fn window_state(&self, state: WindowState) {
        if matches!(state, WindowState::Loading | WindowState::Connected | WindowState::Locked) {
            return;
        }
        let mut st = self.state.lock_or_panic();
        if st.phase != Phase::Ready {
            self.step(&mut st, PhoneDragEvent::PhoneCancel);
        }
    }

    /// The next pointer move with a pending drag starts it: the files the Mac drag offers, `None`
    /// when there is nothing to drag. Every file is requested from the phone now. `button_held` =
    /// the move is a drag of the mouse; without it the Mac drag ends at once, cancelled.
    pub fn take_pending(self: &Arc<Self>, button_held: bool) -> Option<Vec<PhoneDragItem>> {
        let mut st = self.state.lock_or_panic();
        let (clip, blob, dir) = self.checked_pending(&mut st)?;

        let n = clip.filenames.len();
        let stop = CancellationToken::new();
        let last_data = Arc::new(Mutex::new(Instant::now()));
        let items = self.start_receives(&st, &clip, &dir, &blob, &last_data, &stop);

        let offered: Vec<PhoneDragItem> = items.iter().map(|i| i.item.clone()).collect();
        let total = items.iter().map(|i| i.item.size).sum();
        info!(session = %self.session_id, file_count = n, total_bytes = total, "phone drag started");
        let received_files = items.iter().filter(|i| matches!(*i.status.borrow(), ItemStatus::Done(_))).count();

        st.from_phone = Some(FromPhone {
            dir,
            items,
            stop: stop.clone(),
            received_files,
            bytes: 0,
            total,
            released: false,
            posted: None,
            failure: None,
        });

        self.spawn_idle_timer(st.phone_epoch, last_data, stop);
        if !button_held {
            self.drag_cancelled(&mut st);
            return None;
        }
        Some(offered)
    }

    /// The pending drag's files, the window's blob channel and the drag's new staging folder; `None`
    /// (the drag failing, when one was pending) when any is missing.
    fn checked_pending(&self, st: &mut State) -> Option<(ClipData, Arc<BlobChannel>, PathBuf)> {
        let clip = st.pending.take()?;
        let Some(clip) = clip.filter(|c| !c.mime_types.is_empty()) else {
            self.step(st, PhoneDragEvent::NoItems);
            return None;
        };

        let n = clip.filenames.len();
        if clip.data_ids.len() < n || clip.file_sizes.len() < n {
            warn!(
                name_count = n,
                id_count = clip.data_ids.len(),
                size_count = clip.file_sizes.len(),
                "clipData arrays disagree; the drag fails"
            );
            self.step(st, PhoneDragEvent::Failure);
            return None;
        }

        let Some((_, blob)) = st.channels.clone() else {
            debug!(session = %self.session_id, "phone drag fails; the window's channels are not open");
            self.step(st, PhoneDragEvent::Failure);
            return None;
        };

        let dir = self.staging.join(new_guid_lower());
        if let Err(e) = std::fs::create_dir_all(&dir) {
            warn!(error = %e, "drag staging folder not created; the drag fails");
            self.step(st, PhoneDragEvent::Failure);
            return None;
        }

        Some((clip, blob, dir))
    }

    /// Starts receiving each file `clip` offers into `dir` and requests it from the phone, with one
    /// line for the requests not sent and one for the empty files not created.
    fn start_receives(
        self: &Arc<Self>,
        st: &State,
        clip: &ClipData,
        dir: &Path,
        blob: &Arc<BlobChannel>,
        last_data: &Arc<Mutex<Instant>>,
        stop: &CancellationToken,
    ) -> Vec<FromPhoneItem> {
        let n = clip.filenames.len();
        let mut items = Vec::with_capacity(n);
        let mut empty_failed: Option<(usize, std::io::Error)> = None;
        let mut requests_failed: Option<(usize, Option<ChannelError>)> = None;

        for i in 0..n {
            let item = phone_drag_item(clip, i);
            let path = dir.join(i.to_string());
            let (tx, rx) = watch::channel(ItemStatus::Receiving);
            let events = blob.register(item.data_id.clone().into_bytes());

            if item.size == 0
                && let Err(e) = create_empty_file(&path, &tx)
            {
                empty_failed.get_or_insert((0, e)).0 += 1;
            }

            self.runtime.spawn(receive(
                self.clone(),
                blob.clone(),
                item.data_id.clone(),
                path,
                events,
                tx,
                last_data.clone(),
                stop.clone(),
            ));

            if let Err(e) = self.try_send(st, scope::DRAG_REQUEST_ITEM, &wire::drag_request_item(&item.data_id)) {
                requests_failed.get_or_insert((0, e)).0 += 1;
            }
            items.push(FromPhoneItem { item, status: rx, written: false });
        }

        if let Some((failed_count, e)) = requests_failed {
            let error = e.map_or_else(|| "its channel is not open".to_string(), |e| e.to_string());
            warn!(failed_count, %error, route = scope::DRAG_REQUEST_ITEM, "drag messages not sent");
        }
        if let Some((failed_count, e)) = empty_failed {
            warn!(failed_count, error = %e, "empty drag files not created; the destination does not get them");
        }

        items
    }

    pub(super) fn spawn_idle_timer(
        self: &Arc<Self>,
        epoch: u64,
        last_data: Arc<Mutex<Instant>>,
        stop: CancellationToken,
    ) {
        let me = self.clone();
        self.runtime.spawn(async move {
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(IDLE) => {}
                    _ = stop.cancelled() => return,
                }

                let mut st = me.state.lock_or_panic();
                if st.phone_epoch != epoch {
                    return;
                }

                let all_done = st
                    .from_phone
                    .as_ref()
                    .is_none_or(|d| d.items.iter().all(|i| matches!(*i.status.borrow(), ItemStatus::Done(_))));
                if all_done {
                    return;
                }
                if last_data.lock_or_panic().elapsed() >= IDLE {
                    me.give_up(&mut st, "no data from the phone for 10 s".to_string());
                    return;
                }
            }
        });
    }

    /// Bytes of a file of the drag receiving into `dir` arrived.
    pub(super) fn received_bytes(&self, dir: &Path, n: u64) {
        let mut st = self.state.lock_or_panic();
        let Some(d) = st.from_phone.as_mut().filter(|d| d.dir == dir) else { return };
        d.bytes += n;
        let due = d.posted.is_none_or(|t| t.elapsed() >= PROGRESS_INTERVAL);
        if due && st.phase == Phase::Drop {
            self.post_progress(&mut st, false);
        }
    }

    /// A file of the drag receiving into `dir` arrived whole.
    pub(super) fn received_file(&self, dir: &Path) {
        let mut st = self.state.lock_or_panic();
        let Some(d) = st.from_phone.as_mut().filter(|d| d.dir == dir) else { return };
        d.received_files += 1;
        if st.phase == Phase::Drop {
            self.post_progress(&mut st, false);
        }
    }

    pub(super) fn notification_id(&self) -> String {
        format!("{FEATURE}/drag/{}", self.session_id)
    }

    pub(super) fn notify(&self, title: String, body: String, alert: bool, actions: Vec<NotificationAction>) {
        let n = SystemNotification {
            id: self.notification_id(),
            thread_id: Some(format!("{FEATURE}/drag")),
            title,
            subtitle: None,
            body,
            alert,
            actions,
            user_info: HashMap::new(),
            image: None,
        };

        match self.notifier.post(n) {
            Ok(()) => self.notify_failing.store(false, Ordering::Relaxed),
            Err(e) if !self.notify_failing.swap(true, Ordering::Relaxed) => {
                warn!(error = %e, "drag notification not posted; later failures are not logged until one is posted");
            }
            Err(_) => {}
        }
    }

    /// The progress notification of the released drag, with its Cancel action; `first` alerts.
    pub(super) fn post_progress(&self, st: &mut State, first: bool) {
        let Some(d) = st.from_phone.as_mut() else { return };
        d.released = true;
        d.posted = Some(Instant::now());

        let title = progress_title(d.items.len(), d.received_files);
        let body = format!("Downloading… {}", size_text(d.bytes, d.total));
        let cancel =
            NotificationAction { id: ACTION_CANCEL.to_string(), title: "Cancel".to_string(), text_input: None };
        self.notify(title, body, first, vec![cancel]);
    }

    /// The progress notification's Cancel: as the phone's `cancel` with reason 11.
    pub fn cancel_from_notification(&self) {
        self.phone_cancel(reason::USER_CANCELLED);
    }

    /// A received file failed because of `failure`: the Mac drag is cancelled.
    pub(super) fn item_failed(&self, epoch_dir: &Path, failure: String) {
        let mut st = self.state.lock_or_panic();
        if st.from_phone.as_ref().is_some_and(|d| d.dir == epoch_dir) {
            self.give_up(&mut st, failure);
        }
    }

    /// Cancels the phone drag because of `failure`, which its end's line tells.
    fn give_up(&self, st: &mut State, failure: String) {
        if let Some(d) = st.from_phone.as_mut() {
            d.failure.get_or_insert(failure);
        }
        self.drag_cancelled(st);
    }

    /// The Mac drag of the phone's files ended: `dropped` on a destination that took it, else with
    /// none (released outside, refused, Escape).
    pub fn mac_drag_ended(&self, dropped: bool) {
        let mut st = self.state.lock_or_panic();
        let Some(d) = st.from_phone.as_mut() else { return };
        d.released = true;

        if dropped {
            if st.phase == Phase::DragStarted {
                self.step(&mut st, PhoneDragEvent::Dropped);
            }
        } else {
            self.drag_cancelled(&mut st);
        }
    }

    /// The destination asks for file `data_id` at `destination`: waits until it arrived, then puts it
    /// there.
    pub async fn write_item(&self, data_id: &str, destination: &Path) -> Result<(), DragWriteError> {
        let mut status = {
            let mut st = self.state.lock_or_panic();
            let Some(status) = st
                .from_phone
                .as_ref()
                .and_then(|d| d.items.iter().find(|i| i.item.data_id == data_id && !i.written))
                .map(|i| i.status.clone())
            else {
                return Err(DragWriteError::UnknownItem);
            };
            if st.phase == Phase::DragStarted {
                self.step(&mut st, PhoneDragEvent::Dropped);
            }
            status
        };

        let done = status.wait_for(|s| *s != ItemStatus::Receiving).await.map(|s| s.clone());
        let path = match done {
            Ok(ItemStatus::Done(path)) => path,
            _ => return Err(DragWriteError::NotReceived),
        };

        let moved = std::fs::rename(&path, destination)
            .or_else(|_| std::fs::copy(&path, destination).map(|_| ()))
            .map_err(|e| DragWriteError::Io(e.to_string()));

        let mut st = self.state.lock_or_panic();
        if moved.is_err() {
            if st.from_phone.as_ref().is_some_and(|d| d.items.iter().any(|i| i.item.data_id == data_id)) {
                self.drag_cancelled(&mut st);
            }
            return moved;
        }

        if let Some(d) = st.from_phone.as_mut()
            && let Some(i) = d.items.iter_mut().find(|i| i.item.data_id == data_id)
        {
            i.written = true;
            if d.items.iter().all(|i| i.written) && st.phase == Phase::Drop {
                self.step(&mut st, PhoneDragEvent::AllWritten);
            }
        }
        Ok(())
    }
}

/// The `i`th file `clip` offers, as the Mac drag offers it.
fn phone_drag_item(clip: &ClipData, i: usize) -> PhoneDragItem {
    PhoneDragItem {
        data_id: clip.data_ids[i].clone(),
        name: mac_file_name(&clip.filenames[i]),
        size: u64::try_from(clip.file_sizes[i]).unwrap_or(0),
        mime_type: clip.mime_types.get(i).cloned().unwrap_or_default(),
    }
}

/// Creates the empty file of a dragged item at `path`, its status then done, or failed when not created.
fn create_empty_file(path: &Path, tx: &watch::Sender<ItemStatus>) -> std::io::Result<()> {
    match std::fs::File::create(path) {
        Ok(_) => {
            let _ = tx.send(ItemStatus::Done(path.to_path_buf()));
            Ok(())
        }
        Err(e) => {
            let _ = tx.send(ItemStatus::Failed);
            Err(e)
        }
    }
}
