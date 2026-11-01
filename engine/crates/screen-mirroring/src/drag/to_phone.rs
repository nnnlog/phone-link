//! Dragging from the Mac to the phone.

use super::{
    COOL_OFF, DEFAULT_MIME, EnterOutcome, MAX_FILE_BYTES, MAX_ITEMS, MacDragFile, State, ToPhone, WindowDrag, reason,
};
use crate::wire;
use crate::wire::{ClipItem, scope};
use phonelink_core::sync::MutexExt;
use phonelink_core::types::new_guid_lower;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

impl WindowDrag {
    /// A drag of `files` entered the window.
    pub fn enter(self: &Arc<Self>, files: Vec<MacDragFile>) -> EnterOutcome {
        let mut st = self.state.lock_or_panic();
        st.generation += 1;
        st.to_phone = None;
        if st.channels.is_none() {
            debug!(session = %self.session_id, "mac drag refused; the window's channels are not open");
            return EnterOutcome::Refused;
        }

        let (items, paths) = clip_items(files);
        let valid: Vec<&ClipItem> = items.iter().filter(|i| !i.filename.to_lowercase().ends_with(".lnk")).collect();
        if valid.is_empty() {
            return EnterOutcome::Refused;
        }

        let largest = valid.iter().map(|i| i.file_size as u64).max().unwrap_or(0);
        if items.len() > MAX_ITEMS || largest > MAX_FILE_BYTES {
            info!(item_count = items.len(), largest_bytes = largest, "mac drag over the limits; refused");
            return EnterOutcome::OverLimit;
        }

        self.input.lock_or_panic().take_focus();
        // A new drag replaces the earlier one's files.
        self.clear_outgoing(&mut st);
        for (item, path) in items.iter().zip(paths) {
            st.outgoing.insert(item.data_id.clone(), path);
        }

        let correlation_id = new_guid_lower();
        let generation = st.generation;
        let body = wire::drag_start_pc(&correlation_id, &items);
        st.to_phone = Some(ToPhone {
            correlation_id,
            generation,
            started: false,
            drop_pending: false,
            ended: false,
            effect_none: false,
        });

        st.activity = true;
        let wait = st.cool_off_until.map(|t| t.saturating_duration_since(Instant::now())).filter(|d| !d.is_zero());
        match wait {
            None => self.start_pc(&mut st, &body),
            Some(wait) => self.start_pc_after(wait, generation, body),
        }

        EnterOutcome::Accepted
    }

    /// Sends `startPc` once `wait` has passed, when the drag of `generation` still wants it.
    fn start_pc_after(self: &Arc<Self>, wait: Duration, generation: u64, body: Vec<u8>) {
        let me = self.clone();
        self.runtime.spawn(async move {
            tokio::time::sleep(wait).await;
            let mut st = me.state.lock_or_panic();
            let due = st
                .to_phone
                .as_ref()
                .is_some_and(|t| t.generation == generation && !t.started && (!t.ended || t.drop_pending));
            if due {
                me.start_pc(&mut st, &body);
            }
        });
    }

    pub(super) fn start_pc(&self, st: &mut State, body: &[u8]) {
        self.send(st, scope::DRAG_START_PC, body);
        let Some(t) = st.to_phone.as_mut() else { return };
        t.started = true;
        if t.drop_pending {
            let id = t.correlation_id.clone();
            self.send(st, scope::DRAG_DROP_PC, &wire::drag_drop_pc(&id));
        }
    }

    /// The drag is over the window at (`x`, `y`) normalized to the video: `true` while the drag
    /// shows copy. Its position goes to the phone as a mouse move.
    pub fn over(&self, x: f64, y: f64) -> bool {
        let st = self.state.lock_or_panic();
        let Some(t) = &st.to_phone else { return false };
        if t.ended || t.effect_none {
            return false;
        }
        if t.started {
            self.input.lock_or_panic().mouse_move(x, y);
        }
        true
    }

    /// The drag left the window (or was cancelled over it).
    pub fn leave(&self) {
        let mut st = self.state.lock_or_panic();
        st.cool_off_until = Some(Instant::now() + COOL_OFF);
        let Some(t) = st.to_phone.as_mut() else { return };
        if t.ended {
            return;
        }
        t.ended = true;
        if t.effect_none {
            return;
        }

        let (started, id) = (t.started, t.correlation_id.clone());
        if started {
            self.send(&st, scope::DRAG_CANCEL, &wire::drag_cancel(&id, reason::USER_DID_NOT_DROP.into()));
        }

        st.activity = false;
        self.clear_outgoing(&mut st);
    }

    /// The drag was dropped on the window.
    pub fn drop_on_window(&self) {
        let mut st = self.state.lock_or_panic();
        let activity = st.activity;
        let item_count = st.outgoing.len();
        let Some(t) = st.to_phone.as_mut() else { return };
        if t.ended || t.effect_none {
            return;
        }

        t.ended = true;
        info!(session = %self.session_id, item_count, "mac drag dropped");
        if !t.started {
            t.drop_pending = true;
            return;
        }

        let id = t.correlation_id.clone();
        if activity {
            self.send(&st, scope::DRAG_DROP_PC, &wire::drag_drop_pc(&id));
        }
    }

    /// The phone's `requestItem`: the file goes once, as the blob named `item_id`.
    pub(super) fn request_item(self: &Arc<Self>, item_id: String) {
        let (path, blob, generation) = {
            let mut st = self.state.lock_or_panic();
            let Some(path) = st.outgoing.remove(&item_id) else {
                return debug!(item = %item_id, "requestItem for no item of the drag; ignored");
            };
            let Some((_, blob)) = st.channels.clone() else { return };
            st.sending.insert(item_id.clone());
            (path, blob, st.generation)
        };

        let me = self.clone();
        self.runtime.spawn(async move {
            let result = async {
                let file = tokio::fs::File::open(&path).await.map_err(ItemSendError::Read)?;
                let size = file.metadata().await.map_err(ItemSendError::Read)?.len();
                blob.send_stream(item_id.as_bytes(), size, file).await.map_err(ItemSendError::Send)
            }
            .await;

            let mut st = me.state.lock_or_panic();
            st.sending.remove(&item_id);

            // Reason 1 (NoDataFound) ends the Mac side only; the phone is told nothing. Once per drag:
            // the files of one drag fail alike.
            if let Err(e) = result
                && st.send_failure_logged != Some(generation)
            {
                st.send_failure_logged = Some(generation);
                warn!(error = %e, "dragged file not sent; later failures of this drag are not logged");
            }
        });
    }
}

/// Why a dragged file was not sent.
#[derive(Debug, thiserror::Error)]
enum ItemSendError {
    #[error("{0}")]
    Read(#[source] std::io::Error),
    #[error("{0}")]
    Send(#[source] phonelink_nano::channel::blob::BlobSendError),
}

/// The clip items of the dragged `files` that can be sent, with their paths; unreadable files, folders
/// and paths without a name are skipped.
fn clip_items(files: Vec<MacDragFile>) -> (Vec<ClipItem>, Vec<PathBuf>) {
    let mut items = Vec::new();
    let mut paths = Vec::new();
    let mut unreadable: Option<(usize, std::io::Error)> = None;
    let mut unnamed_count = 0usize;

    for f in files {
        let meta = match std::fs::metadata(&f.path) {
            Ok(m) => m,
            Err(e) => {
                unreadable.get_or_insert((0, e)).0 += 1;
                continue;
            }
        };

        if meta.is_dir() {
            continue;
        }
        let Some(name) = f.path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            unnamed_count += 1;
            continue;
        };

        items.push(ClipItem {
            mime_type: f.mime_type.unwrap_or_else(|| DEFAULT_MIME.to_string()),
            filename: name,
            file_size: i64::try_from(meta.len()).unwrap_or(i64::MAX),
            data_id: new_guid_lower(),
        });
        paths.push(f.path);
    }

    if let Some((unreadable_count, e)) = &unreadable {
        warn!(unreadable_count, error = %e, "dragged files cannot be read; skipped");
    }
    if unnamed_count > 0 {
        warn!(unnamed_count, "dragged paths without a name; skipped");
    }

    (items, paths)
}
