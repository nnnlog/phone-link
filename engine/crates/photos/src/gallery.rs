//! The gallery and the viewer: what they show and what the user does there.

use super::{PhoneState, PhotoItem, Photos, PhotosError, PhotosStatus, SaveAsName, save_as_name, store, trace_name};
use phonelink_core::sync::MutexExt;
use std::sync::Arc;

impl Photos {
    /// The feature's state for the selected phone.
    pub fn status(&self) -> Result<PhotosStatus, PhotosError> {
        let (phone, ps) = self.current()?;
        let st = store::state(&ps.db)?;
        let syncing = ps.ui.lock_or_panic().indicator.updating;
        Ok(self.inner.status_of(&phone, self.inner.record().as_ref(), &st, syncing))
    }

    /// The gallery in display order (`taken DESC, last-modified DESC`).
    pub fn gallery(&self) -> Result<Vec<PhotoItem>, PhotosError> {
        let (_, ps) = self.current()?;
        self.displayed(&ps)
    }

    pub fn item(&self, id: i64) -> Result<Option<PhotoItem>, PhotosError> {
        let (_, ps) = self.current()?;
        let state = ps.ui.lock_or_panic().deleting.get(&id).copied().unwrap_or_default();
        Ok(store::item(&ps.db, id)?.map(|r| PhotoItem::from_row(r, state)))
    }

    pub fn thumbnail(&self, id: i64) -> Result<Option<Vec<u8>>, PhotosError> {
        let (_, ps) = self.current()?;
        Ok(store::thumbnail(&ps.db, id)?)
    }

    /// The cached full-resolution bytes (no fetch).
    pub fn full_image(&self, id: i64) -> Result<Option<Vec<u8>>, PhotosError> {
        let (_, ps) = self.current()?;
        Ok(store::full_bytes(&ps.db, id)?)
    }

    /// The default save-as name of an item.
    pub fn save_as_name(&self, id: i64) -> Result<SaveAsName, PhotosError> {
        let (_, ps) = self.current()?;
        Ok(save_as_name(&self.row(&ps, id)?.name))
    }

    // --- triggers -----------------------------------------------------------------------------

    /// A gallery trigger: marks the fetches pending and queues them under `trigger`.
    pub(super) fn mark_and_fetch_full(&self, ps: &Arc<PhoneState>, ids: &[i64], trigger: &'static str) {
        if ids.is_empty() {
            return;
        }
        ps.ui.lock_or_panic().pending_full.extend(ids.iter().copied());
        self.inner.request_full(ps, ids, trigger);
    }

    /// An item became visible: without thumbnail or full bytes, its thumbnail is fetched (one id per
    /// trigger, batched by the dispatcher). Nothing with optimized thumbnails.
    pub fn on_item_visible(&self, id: i64) -> Result<(), PhotosError> {
        if self.inner.config.media_optimized_thumbnails {
            return Ok(());
        }

        let (_, ps) = self.current()?;
        let Some(row) = store::item(&ps.db, id)? else { return Ok(()) };
        if !row.has_thumbnail && !row.has_full {
            ps.ui.lock_or_panic().pending_thumbnails.insert(id);
            self.inner.request_thumbnails(&ps, &[id], trace_name::THUMBNAIL_REQUESTED);
        }
        Ok(())
    }

    /// A grid layout pass with these visible items: with optimized thumbnails, one batch for
    /// every one of them lacking an image. Nothing otherwise.
    pub fn on_grid_layout(&self, visible: &[i64]) -> Result<(), PhotosError> {
        if !self.inner.config.media_optimized_thumbnails {
            return Ok(());
        }
        let (_, ps) = self.current()?;

        let mut batch = Vec::new();
        for id in visible {
            if let Some(row) = store::item(&ps.db, *id)? {
                if !row.has_thumbnail && !row.has_full {
                    batch.push(*id);
                }
            }
        }

        if !batch.is_empty() {
            ps.ui.lock_or_panic().pending_thumbnails.extend(batch.iter().copied());
            self.inner.request_thumbnails(&ps, &batch, trace_name::MEDIA_VIEWED_IN_NODE);
        }
        Ok(())
    }

    /// An item scrolled out of view: the fetches the gallery requested for it are cancelled.
    pub fn on_item_hidden(&self, id: i64) -> Result<(), PhotosError> {
        let (_, ps) = self.current()?;
        let (thumb, full) = {
            let mut ui = ps.ui.lock_or_panic();
            (ui.pending_thumbnails.remove(&id), ui.pending_full.remove(&id))
        };

        if thumb {
            self.inner.cancel_thumbnails(&ps, &[id]);
        }
        if full {
            self.inner.cancel_full(&ps, &[id]);
        }
        Ok(())
    }

    /// The gallery was left: every queued fetch of the phone is cancelled.
    pub fn on_gallery_left(&self) -> Result<(), PhotosError> {
        let (_, ps) = self.current()?;
        self.inner.cancel_all_thumbnails(&ps);
        self.inner.cancel_all_full(&ps);
        let mut ui = ps.ui.lock_or_panic();
        ui.pending_thumbnails.clear();
        ui.pending_full.clear();
        Ok(())
    }

    /// A grid item was clicked: when it shows an image (thumbnail or full bytes), its full bytes are
    /// fetched if missing and `true` says to open the viewer. An item a delete took out of the
    /// displayed gallery is no grid item: `false`, nothing fetched.
    pub fn open_item(&self, id: i64) -> Result<bool, PhotosError> {
        let (_, ps) = self.current()?;
        let row = self.row(&ps, id)?;
        if !(row.has_thumbnail || row.has_full) || self.is_hidden(&ps, id) {
            return Ok(false);
        }
        if !row.has_full {
            self.mark_and_fetch_full(&ps, &[id], trace_name::IMAGE_REQUESTED);
        }
        Ok(true)
    }

    /// The viewer's selection changed (on opening and on every flip): the selected item, then the
    /// next, the previous, the second next and the second previous item, each only without full
    /// bytes, are fetched — the selected one on top.
    pub fn on_viewer_selection(&self, id: i64) -> Result<(), PhotosError> {
        let (_, ps) = self.current()?;
        let list = self.displayed(&ps)?;
        let Some(at) = list.iter().position(|i| i.id == id) else { return Ok(()) };

        let mut window = Vec::new();
        let mut add = |index: Option<usize>| {
            if let Some(item) = index.and_then(|i| list.get(i)) {
                if !item.has_full {
                    window.push(item.id);
                }
            }
        };

        add(Some(at));
        for k in 1..=self.inner.config.viewer_neighbours {
            add(at.checked_add(k));
            add(at.checked_sub(k));
        }

        self.mark_and_fetch_full(&ps, &window, trace_name::MEDIA_VIEWED_IN_GALLERY);
        Ok(())
    }

    /// The viewer shows this item: its last-seen time is stamped.
    pub fn on_viewer_item_shown(&self, id: i64) -> Result<(), PhotosError> {
        let (_, ps) = self.current()?;
        store::set_last_seen(&ps.db, id, self.inner.now_ms())?;
        Ok(())
    }

    /// The viewer closed: the full-resolution fetches the gallery requested are cancelled.
    pub fn on_viewer_closed(&self) -> Result<(), PhotosError> {
        let (_, ps) = self.current()?;
        let ids: Vec<i64> = ps.ui.lock_or_panic().pending_full.drain().collect();
        self.inner.cancel_full(&ps, &ids);
        Ok(())
    }

    /// An item's context menu opened (whether or not an action is chosen): its full bytes are fetched
    /// if missing — also after an earlier fetch was cancelled or gave up. An item a delete took out of
    /// the displayed gallery has no menu: nothing is fetched.
    pub fn on_context_menu_opened(&self, id: i64) -> Result<(), PhotosError> {
        let (_, ps) = self.current()?;
        if !self.row(&ps, id)?.has_full && !self.is_hidden(&ps, id) {
            self.mark_and_fetch_full(&ps, &[id], trace_name::IMAGE_REQUESTED);
        }
        Ok(())
    }
}
