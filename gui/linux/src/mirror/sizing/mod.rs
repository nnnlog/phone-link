//! How a mirroring window is sized and placed. The window's video area keeps the video's aspect; the
//! window adds its toolbar above it, and the navigation bar under it while that shows (`non_client`,
//! what the window adds to the video area each way). The window manager's title bar is outside the
//! window's own size.
//!
//! Sizes and rectangles are in logical pixels, rectangles with their origin at the top left, y
//! downwards; `visible` is the room a window has on its screen. Lengths are rounded to whole pixels, a
//! half to the even one.
//!
//! - A new window without a kept frame fits three quarters of the visible room's width and height with
//!   the video's aspect (`default_size`); the window manager places it.
//! - A kept frame is used when it overlaps a screen (`restored`); a window put at a place then steps 32
//!   pixels down and right while another mirroring window on its screen has its top-left corner within
//!   4 pixels of its own, at most 8 times and only while it stays on that screen (`cascaded`).
//! - A video of a new aspect reshapes the window (`reshaped`): not when the video area's aspect is within
//!   0.01 of it; else the size the window had before its last reshape returns when it had that aspect
//!   and fits the visible room; else the video area takes the video's own size — a device pixel per
//!   video pixel — between the smallest window and three quarters of the largest window of that aspect.
//! - The navigation bar appearing or going adds or takes its height (`height_changed`), unless the
//!   window already holds it (`holds_navigation_bar`).
//!
//! The windows ask through the `mirror.frame` query (`request`).

mod geometry;
mod placement;
mod request;
mod sizes;

pub use request::answer;

#[cfg(test)]
mod tests;
