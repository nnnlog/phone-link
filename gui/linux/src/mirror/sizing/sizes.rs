//! A window's size: a new window's, the smallest and largest of an aspect, the size for a video of a
//! new aspect, and the height with the navigation bar.

use super::geometry::{Placement, Size, clamp, round};

/// A new window's share of the visible room each way, and the largest share a reshape gives it.
const DEFAULT_SHARE: f64 = 0.75;
/// Aspects closer than this are the same.
const ASPECT_TOLERANCE: f64 = 0.01;
/// The smallest window: this wide for a portrait video (aspect at most 1)…
const MINIMUM_WIDTH: f64 = 240.0;
/// …and this high for a landscape one.
const MINIMUM_HEIGHT: f64 = 200.0;
/// The navigation bar under the video.
const NAVIGATION_BAR_HEIGHT: f64 = 32.0;

/// The aspect of a window's video area, or `None` when it has no height.
pub(super) fn content_aspect(size: Size, non_client: Size) -> Option<f64> {
    let height = size.height - non_client.height;
    (height > 0.0).then(|| (size.width - non_client.width) / height)
}

pub(super) fn same_aspect(a: f64, b: f64) -> bool {
    (a - b).abs() <= ASPECT_TOLERANCE
}

/// A new window's size for a video of `aspect` (width ÷ height).
pub(super) fn default_size(aspect: f64, visible: Size, non_client: Size) -> Size {
    let room_box = Size::new(round(visible.width * DEFAULT_SHARE), round(visible.height * DEFAULT_SHARE));
    let room = Size::new(room_box.width - non_client.width, room_box.height - non_client.height);
    let room_aspect = if room.height > 0.0 { room.width / room.height } else { aspect };

    if aspect <= room_aspect {
        Size::new(round(room.height * aspect) + non_client.width, room_box.height)
    } else {
        Size::new(room_box.width, round(room.width / aspect) + non_client.height)
    }
}

/// The smallest window for a video of `aspect`.
///
/// It is 240 pixels wide for a portrait video (the height from the aspect, at most the visible height),
/// 200 pixels high for a landscape one (the width from the aspect, at least 240 pixels and at most the
/// visible width).
pub(super) fn minimum_size(aspect: f64, visible: Size, non_client: Size) -> Size {
    if aspect <= 1.0 {
        let height = round((MINIMUM_WIDTH - non_client.width) / aspect) + non_client.height;
        return Size::new(MINIMUM_WIDTH, visible.height.min(height));
    }
    let width = round((MINIMUM_HEIGHT - non_client.height) * aspect) + non_client.width;
    Size::new(visible.width.min(MINIMUM_WIDTH.max(width)), MINIMUM_HEIGHT)
}

/// The largest window of `aspect` in the visible room: its width when the window is as high as the
/// room, its height when it is as wide, each between the smallest window and the room.
fn maximum_size(aspect: f64, visible: Size, non_client: Size) -> Size {
    let minimum = minimum_size(aspect, visible, non_client);
    let width = round((visible.height - non_client.height) * aspect) + non_client.width;
    let height = round((visible.width - non_client.width) / aspect) + non_client.height;
    Size::new(clamp(width, minimum.width, visible.width), clamp(height, minimum.height, visible.height))
}

/// The window size for a video of `content`: its own size where that lies between the smallest window
/// and three quarters of the largest one of its aspect, else the nearer of the two.
fn reshaped_size(content: Size, visible: Size, non_client: Size) -> Size {
    let aspect = content.width / content.height;
    let minimum = minimum_size(aspect, visible, non_client);
    let maximum = maximum_size(aspect, visible, non_client);

    let low = Size::new(minimum.width - non_client.width, minimum.height - non_client.height);
    let mut high = Size::new(
        round(maximum.width * DEFAULT_SHARE) - non_client.width,
        round(maximum.height * DEFAULT_SHARE) - non_client.height,
    );
    if high.width < low.width {
        high.width = low.width.max(maximum.width - non_client.width);
    }
    if high.height < low.height {
        high.height = low.height.max(maximum.height - non_client.height);
    }

    let lowest = (low.width / content.width).max(low.height / content.height);
    let highest = (high.width / content.width).min(high.height / content.height);
    let scale = if lowest < highest { 1f64.max(lowest).min(highest) } else { highest };

    let width = clamp(round(content.width * scale), low.width, high.width);
    let height = clamp(round(content.height * scale), low.height, high.height);
    Size::new(width + non_client.width, height + non_client.height)
}

/// The size of a window now `current` for a video of `content` pixels (a device pixel per video pixel,
/// in logical pixels).
///
/// It is `None` when the window's video area is already within 0.01 of the video's aspect.
/// `fallback_aspect` stands for the window's when its video area has no height; `previous` is the size
/// the window had before its last reshape.
pub(super) fn reshaped(
    content: Size,
    current: Size,
    fallback_aspect: f64,
    previous: Option<Placement>,
    visible: Size,
    non_client: Size,
) -> Option<Size> {
    if content.width <= 0.0 || content.height <= 0.0 {
        return None;
    }
    let aspect = content.width / content.height;
    let current_aspect = content_aspect(current, non_client).unwrap_or(fallback_aspect);
    if same_aspect(aspect, current_aspect) {
        return None;
    }

    if let Some(previous) = previous
        && same_aspect(previous.aspect, aspect)
        && previous.size.width <= visible.width
        && previous.size.height <= visible.height
    {
        return Some(previous.size);
    }
    Some(reshaped_size(content, visible, non_client))
}

/// Whether a window of `size` already holds the navigation bar: its video area without the bar's
/// height is within 0.01 of `aspect`, and nearer to it than with that height.
pub(super) fn holds_navigation_bar(size: Size, non_client_without_bar: Size, aspect: f64) -> bool {
    let width = size.width - non_client_without_bar.width;
    let height = size.height - non_client_without_bar.height;
    let without = height - NAVIGATION_BAR_HEIGHT;
    if width <= 0.0 || without <= 0.0 {
        return false;
    }
    let apart = (width / without - aspect).abs();
    apart <= ASPECT_TOLERANCE && apart < (width / height - aspect).abs()
}

/// `size` made `delta` pixels higher (lower when negative), its width kept, its height between the
/// smallest and the largest window of `aspect`.
pub(super) fn height_changed(size: Size, delta: f64, aspect: f64, visible: Size, non_client: Size) -> Size {
    let minimum = minimum_size(aspect, visible, non_client);
    let maximum = maximum_size(aspect, visible, non_client);
    Size::new(size.width, clamp(size.height + delta, minimum.height, maximum.height))
}
