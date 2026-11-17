//! The photo a file action works on: its file name and type, checked before the action starts, its full
//! image, fetched for the action, and the title of the action's failure.

use std::sync::Arc;

use phonelink_engine::api::engine::Engine;
use phonelink_engine::api::photos::{PhotoAction, PhotoFileName};
use tracing::{error, info};

use super::files;
use crate::apps_host::{self, desktop};
use crate::l10n::{fill, translate};
use crate::user_text;

/// A photo's full image on its way for a file action: the image, or why it did not come (in the words
/// the window shows).
pub(super) type Fetching = tokio::task::JoinHandle<Result<Vec<u8>, String>>;

/// The MIME type of a save-as file name.
pub(super) fn mime_of(file_name: &str) -> Option<String> {
    apps_host::mime_type_of_name(file_name)
}

/// Open, Open With and drag-out hand the file to the application its type names.
pub(super) fn hands_file_to_app(action: PhotoAction) -> bool {
    matches!(action, PhotoAction::Open | PhotoAction::OpenWith | PhotoAction::DragOut)
}

pub(super) fn failure_title(action: PhotoAction) -> &'static str {
    match action {
        PhotoAction::Copy | PhotoAction::DragOut => "Couldn't Copy the Photo",
        PhotoAction::SaveAs => "Couldn't Save the Photo",
        PhotoAction::Share => "Couldn't Share the Photo",
        PhotoAction::Open | PhotoAction::OpenWith => "Couldn't Open the Photo",
    }
}

/// A file action's photo: its save-as name and MIME type.
pub(super) struct ActionFile {
    pub(super) id: i64,
    pub(super) name: PhotoFileName,
    pub(super) file_name: String,
    pub(super) mime: Option<String>,
}

/// The file a file action gets of photo `id`, or why it gets none (in the words the window shows):
/// a name that is no file name, or — for the actions that hand the file to an application — one that
/// is no photo's or video's, since the name comes from the phone.
pub(super) fn action_file(engine: &Arc<Engine>, id: i64, action: PhotoAction) -> Result<ActionFile, String> {
    let name = engine.clone().photos().save_as_name(id).map_err(|e| user_text::feature_error(&e))?;
    let file_name = files::file_name(&name)?;
    let mime = mime_of(&file_name);

    if hands_file_to_app(action) && !mime.as_deref().is_some_and(desktop::is_media) {
        info!(id, ?mime, "a photo is not handed to an application; its name is no photo's or video's");
        return Err(fill(translate("\u{201c}%@\u{201d} isn't a photo or video file."), &[file_name]));
    }
    Ok(ActionFile { id, name, file_name, mime })
}

/// The full image for a file action: fetched when not cached, waited for at most 30 s.
pub(super) async fn fetch(engine: Arc<Engine>, id: i64, action: PhotoAction) -> Result<Vec<u8>, String> {
    match engine.photos().full_image_for_action(id, action).await {
        Ok(Some(data)) => Ok(data),
        Ok(None) => {
            error!(id, "the full image of a photo did not arrive within 30 s");
            Err(translate("Your phone didn't respond.").to_owned())
        }
        Err(e) => {
            error!(id, error = %e, "the full image of a photo was not fetched");
            Err(user_text::feature_error(&e))
        }
    }
}
