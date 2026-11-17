//! Drag out: the file a photo drags out as, written from its cached full image, which is fetched first
//! when it is not here yet.

use std::pin::Pin;
use std::sync::Arc;

use phonelink_engine::api::engine::Engine;
use phonelink_engine::api::photos::PhotoAction;
use serde_json::Value;
use tracing::{info, warn};

use super::action_file::action_file;
use super::files;
use crate::engine_bridge::EngineBridge;
use crate::images;

impl EngineBridge {
    /// The file a photo drags out as, written now from the cached full image; `null` when the full
    /// image is not cached yet, or the photo's name is no photo's or video's.
    pub(super) fn photos_drag_file(&self, engine: &Arc<Engine>, arg: &str) -> Value {
        let Ok(id) = arg.parse::<i64>() else { return Value::Null };
        let Ok(file) = action_file(engine, id, PhotoAction::DragOut) else { return Value::Null };
        let Some(data) = engine.clone().photos().full_image(id).ok().flatten() else { return Value::Null };

        match files::write_photo(id, &file.file_name, &data) {
            Ok(path) => Value::from(images::file_url(&path)),
            Err(e) => {
                warn!(id, error = %e, "a photo was not written to drag out");
                Value::Null
            }
        }
    }

    /// A drag of a photo whose full image is not here yet: it is fetched, and the photo drags out once
    /// it arrives.
    pub(super) fn prepare_drag(mut self: Pin<&mut Self>, engine: Arc<Engine>, id: i64) {
        if action_file(&engine, id, PhotoAction::DragOut).is_err() {
            return;
        }

        let task = self.as_mut().fetch_marked(engine, id, PhotoAction::DragOut);
        self.runtime.spawn(async move {
            if let Ok(Err(e)) = task.await {
                info!(id, error = %e, "a photo did not arrive to drag out");
            }
        });
    }
}
