//! The grid and the viewer: the cells on screen, the gallery entered and left, the selection, the
//! viewer's photo, and the photos waiting on the phone, each change announced as the `ui` event.

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use cxx_qt::CxxQtType;
use phonelink_engine::api::engine::Engine;
use serde_json::Value;
use tracing::{debug, info, warn};

use super::gallery::gallery;
use super::{files, quiet};
use crate::engine_bridge::EngineBridge;
use crate::engine_bridge::features::Failure;
use crate::user_text;

impl EngineBridge {
    pub(super) fn photos_ui_changed(mut self: Pin<&mut Self>) {
        let payload = self.areas.photos.ui_json();
        self.as_mut().emit_area_event("photos", "ui", &payload);
    }

    pub(super) fn mark_busy(mut self: Pin<&mut Self>, ids: &[i64], busy: bool) {
        let counts = &mut self.as_mut().rust_mut().areas.photos.busy;
        for id in ids {
            if busy {
                *counts.entry(*id).or_default() += 1;
            } else if let Some(count) = counts.get_mut(id) {
                *count -= 1;
                if *count == 0 {
                    counts.remove(id);
                }
            }
        }

        self.photos_ui_changed();
    }

    /// A grid cell appeared (the engine fetches its thumbnail when missing) or disappeared (the engine
    /// cancels the fetches queued for it).
    pub(super) fn cell_shown(mut self: Pin<&mut Self>, engine: &Arc<Engine>, id: i64, shown: bool) {
        let visible = &mut self.as_mut().rust_mut().areas.photos.visible;
        let result = if shown {
            visible.insert(id);
            engine.clone().photos().item_visible(id)
        } else {
            visible.remove(&id);
            engine.clone().photos().item_hidden(id)
        };

        if let Err(e) = result
            && !quiet(&e)
        {
            info!(error = %e, "a grid cell was not passed on");
        }
    }

    /// The gallery left the screen (another section, or the window closed): every queued fetch is
    /// cancelled and the viewer closes.
    pub(super) fn gallery_left(mut self: Pin<&mut Self>, engine: &Arc<Engine>) {
        let state = &mut self.as_mut().rust_mut().areas.photos;
        state.gallery_left = true;
        state.departures.fetch_add(1, Ordering::SeqCst);

        if let Err(e) = engine.clone().photos().gallery_left()
            && !quiet(&e)
        {
            info!(error = %e, "leaving the photos was not passed on");
        }

        self.close_viewer(engine);
    }

    /// The gallery is on screen again: the cells on screen are reported again, since the fetches queued
    /// for them were cancelled, and Open With reads the applications again.
    pub(super) fn gallery_entered(mut self: Pin<&mut Self>, engine: &Arc<Engine>) {
        let state = &mut self.as_mut().rust_mut().areas.photos;
        state.open_with.get_mut().clear();
        if !std::mem::take(&mut state.gallery_left) {
            return;
        }

        for id in &state.visible {
            if let Err(e) = engine.clone().photos().item_visible(*id)
                && !quiet(&e)
            {
                info!(error = %e, "a grid cell was not passed on");
            }
        }
    }

    pub(super) fn change_selection(mut self: Pin<&mut Self>, engine: &Arc<Engine>, name: &str, arg: &str) {
        let order: Vec<i64> = gallery(engine).iter().map(|photo| photo.id).collect();
        let listed = |id: i64| {
            let known = order.contains(&id);
            if !known {
                debug!(action = name, "the action on a photo the gallery no longer lists is ignored");
            }
            known
        };

        let before = self.areas.photos.selection.focus();
        {
            let selection = &mut self.as_mut().rust_mut().areas.photos.selection;
            match name {
                "photos.select" => {
                    let request: Value = serde_json::from_str(arg).unwrap_or(Value::Null);
                    let id = request["id"].as_i64();
                    match (id, request["mode"].as_str()) {
                        (None, _) => selection.select(None),
                        (Some(id), _) if !listed(id) => {}
                        (Some(id), Some("toggle")) => selection.toggle(id),
                        (Some(id), Some("extend")) => selection.extend(&order, id),
                        (Some(id), _) => selection.select(Some(id)),
                    }
                }
                "photos.selectAll" => selection.select_all(&order),
                "photos.move" => match arg.parse::<i64>() {
                    Ok(offset) => selection.move_by(&order, offset),
                    Err(e) => warn!(error = %e, "photos.move takes an offset"),
                },
                _ => match arg.parse::<i64>() {
                    // A right-click: a photo outside the selection becomes the selection, and the engine
                    // fetches its full image ahead of an action.
                    Ok(id) if listed(id) => {
                        if !selection.contains(id) {
                            selection.select(Some(id));
                        }
                        if let Err(e) = engine.clone().photos().context_menu_opened(id)
                            && !quiet(&e)
                        {
                            info!(error = %e, "an opened context menu was not passed on");
                        }
                    }
                    Ok(_) => {}
                    Err(e) => warn!(error = %e, "photos.contextMenu takes a photo id"),
                },
            }
        }

        let focus = self.areas.photos.selection.focus();
        if self.areas.photos.viewer.is_some() && focus != before {
            match focus {
                Some(id) => self.as_mut().show_in_viewer(engine, id),
                None => self.as_mut().close_viewer(engine),
            }
        }

        self.photos_ui_changed();
    }

    /// Opens the viewer on photo `id` (double-click, Space) when it shows an image; the engine fetches
    /// its full image, and the viewer shows the thumbnail until it arrives.
    pub(super) fn open_viewer(mut self: Pin<&mut Self>, engine: &Arc<Engine>, id: i64) {
        let api = engine.clone().photos();
        match api.open_item(id) {
            Ok(true) => {}
            Ok(false) => return,
            Err(e) => {
                if !quiet(&e) {
                    warn!(id, error = %e, "a photo was not opened in the viewer");
                }
                return;
            }
        }

        if let Err(message) =
            api.save_as_name(id).map_err(|e| user_text::feature_error(&e)).and_then(|n| files::file_name(&n))
        {
            self.as_mut().show_failure(Failure::new("Couldn't Show the Photo", message));
            return;
        }

        {
            let state = &mut self.as_mut().rust_mut().areas.photos;
            if state.selection.contains(id) {
                state.selection.focus_on(id);
            } else {
                state.selection.select(Some(id));
            }
            state.viewer = Some(id);
        }
        self.as_mut().show_in_viewer(engine, id);
        self.photos_ui_changed();
    }

    /// The viewer moved to `id`: the engine prefetches it and its neighbors and stamps it seen.
    fn show_in_viewer(mut self: Pin<&mut Self>, engine: &Arc<Engine>, id: i64) {
        self.as_mut().rust_mut().areas.photos.viewer = Some(id);

        let api = engine.clone().photos();
        for result in [api.viewer_selection(id), api.viewer_item_shown(id)] {
            if let Err(e) = result
                && !quiet(&e)
            {
                info!(error = %e, "the viewer's photo was not passed on");
            }
        }
    }

    pub(super) fn close_viewer(mut self: Pin<&mut Self>, engine: &Arc<Engine>) {
        if self.as_mut().rust_mut().areas.photos.viewer.take().is_none() {
            return;
        }

        if let Err(e) = engine.clone().photos().viewer_closed()
            && !quiet(&e)
        {
            info!(error = %e, "the closed viewer was not passed on");
        }

        self.photos_ui_changed();
    }
}
