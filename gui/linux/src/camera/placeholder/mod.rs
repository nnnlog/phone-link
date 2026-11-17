//! The still images the camera shows while no video does: a dark frame with a line of text under the
//! middle, and for the connecting animation a ring of 16 ticks above it, one image per tick.

use std::process::Command;
use std::sync::OnceLock;

use ab_glyph::{Font, FontVec, PxScale, ScaleFont, point};
use phonelink_connected_camera::CameraPlaceholder;

use super::frame::{FRAME_BYTES, HEIGHT, WIDTH};

/// Images of the connecting animation, and ticks of its spinner.
pub(super) const CONNECTING_FRAMES: usize = 16;

const BACKGROUND_LUMA: u8 = 40;
const TEXT_LUMA: f32 = 235.0;
const TEXT_SIZE: f32 = 40.0;
/// The top of the text's line, 50 pixels under the middle.
const TEXT_TOP: f32 = HEIGHT as f32 / 2.0 + 50.0;
/// The spinner's center, 50 pixels over the middle, and its ticks: from 34 to 64 pixels out, 12 wide
/// with round ends.
const SPINNER_CENTER: (f64, f64) = (WIDTH as f64 / 2.0, HEIGHT as f64 / 2.0 - 50.0);
const TICK_INNER: f64 = 34.0;
const TICK_OUTER: f64 = 64.0;
const TICK_HALF_WIDTH: f64 = 6.0;

fn font() -> Option<&'static FontVec> {
    static FONT: OnceLock<Option<FontVec>> = OnceLock::new();
    FONT.get_or_init(|| {
        let output = Command::new("fc-match").args(["-f", "%{file}\n%{index}", "sans-serif:lang=ko"]).output().ok()?;
        let text = String::from_utf8(output.stdout).ok()?;
        let mut lines = text.lines();
        let path = lines.next()?;
        let index = lines.next().and_then(|i| i.parse().ok()).unwrap_or(0);
        let data = std::fs::read(path).ok()?;
        FontVec::try_from_vec_and_index(data, index)
            .inspect_err(|e| tracing::warn!(path, error = %e, "the file fc-match named is no font"))
            .ok()
    })
    .as_ref()
}

fn text(placeholder: CameraPlaceholder) -> Option<&'static str> {
    let english = match placeholder {
        CameraPlaceholder::Black => return None,
        CameraPlaceholder::Connecting => "Connecting to your phone...",
        CameraPlaceholder::Paused => "Paused",
        CameraPlaceholder::PhoneLocked => "Check your phone",
        CameraPlaceholder::Disconnected => "Your phone is disconnected",
        CameraPlaceholder::StartStreamNotification => "Tap the notification on your phone to start",
        CameraPlaceholder::Error => "The camera stopped",
    };
    Some(crate::l10n::translate(english))
}

fn blank(luma: u8) -> Vec<u8> {
    let mut frame = vec![luma; WIDTH * HEIGHT];
    frame.resize(FRAME_BYTES, 128);
    frame
}

fn blend(frame: &mut [u8], x: i64, y: i64, coverage: f32) {
    let (Ok(x), Ok(y)) = (usize::try_from(x), usize::try_from(y)) else { return };
    if x >= WIDTH || y >= HEIGHT {
        return;
    }
    let pixel = &mut frame[y * WIDTH + x];
    let coverage = coverage.clamp(0.0, 1.0);
    *pixel = (f32::from(*pixel) * (1.0 - coverage) + TEXT_LUMA * coverage) as u8;
}

/// Draws `text` centered, its line's top at `top`, smaller when it would not fit 80 pixels in from each
/// side.
fn draw_text(frame: &mut [u8], font: &FontVec, text: &str, top: f32) {
    let width_of = |size: f32| {
        let scaled = font.as_scaled(PxScale::from(size));
        text.chars().map(|c| scaled.h_advance(scaled.glyph_id(c))).sum::<f32>()
    };
    let limit = WIDTH as f32 - 160.0;
    let size = if width_of(TEXT_SIZE) > limit { TEXT_SIZE * limit / width_of(TEXT_SIZE) } else { TEXT_SIZE };

    let scaled = font.as_scaled(PxScale::from(size));
    let baseline = top + scaled.ascent();
    let mut x = (WIDTH as f32 - width_of(size)) / 2.0;
    for c in text.chars() {
        let id = scaled.glyph_id(c);
        let glyph = id.with_scale_and_position(size, point(x, baseline));
        x += scaled.h_advance(id);
        let Some(outline) = font.outline_glyph(glyph) else { continue };
        let bounds = outline.px_bounds();
        outline.draw(|gx, gy, coverage| {
            blend(frame, bounds.min.x as i64 + i64::from(gx), bounds.min.y as i64 + i64::from(gy), coverage);
        });
    }
}

/// How far the point (`x`, `y`) is from the segment from `start` to `end`.
fn distance_to_segment((x, y): (f64, f64), start: (f64, f64), end: (f64, f64)) -> f64 {
    let (dx, dy) = (end.0 - start.0, end.1 - start.1);
    let along = (((x - start.0) * dx + (y - start.1) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
    (x - start.0 - along * dx).hypot(y - start.1 - along * dy)
}

/// The tick of the spinner at `tick`, clockwise from the top: its inner and outer ends.
fn tick_ends(tick: usize) -> ((f64, f64), (f64, f64)) {
    let angle = tick as f64 * std::f64::consts::TAU / CONNECTING_FRAMES as f64;
    let (along_x, along_y) = (angle.sin(), -angle.cos());
    let at = |radius: f64| (SPINNER_CENTER.0 + along_x * radius, SPINNER_CENTER.1 + along_y * radius);
    (at(TICK_INNER), at(TICK_OUTER))
}

/// How bright the tick at `tick` is in image `step`: the one at the step fully, each one before it 6% less,
/// down to 20%.
fn tick_strength(tick: usize, step: usize) -> f64 {
    let age = (step % CONNECTING_FRAMES + CONNECTING_FRAMES - tick) % CONNECTING_FRAMES;
    (1.0 - age as f64 * 0.06).max(0.2)
}

/// The connecting animation's spinner in image `step`: a tick per image, the one at `step` brightest.
fn draw_spinner(frame: &mut [u8], step: usize) {
    for tick in 0..CONNECTING_FRAMES {
        let (inner, outer) = tick_ends(tick);
        let strength = tick_strength(tick, step);
        let reach = TICK_HALF_WIDTH + 1.0;
        let (left, right) = ((inner.0.min(outer.0) - reach) as i64, (inner.0.max(outer.0) + reach) as i64);
        let (top, bottom) = ((inner.1.min(outer.1) - reach) as i64, (inner.1.max(outer.1) + reach) as i64);
        for y in top..=bottom {
            for x in left..=right {
                let distance = distance_to_segment((x as f64 + 0.5, y as f64 + 0.5), inner, outer);
                let coverage = (TICK_HALF_WIDTH + 0.5 - distance).clamp(0.0, 1.0);
                if coverage > 0.0 {
                    blend(frame, x, y, (coverage * strength) as f32);
                }
            }
        }
    }
}

/// The frame that shows `placeholder`, at image `step` of the connecting animation.
pub(super) fn frame(placeholder: CameraPlaceholder, step: usize) -> Vec<u8> {
    let Some(text) = text(placeholder) else { return blank(16) };
    let mut frame = blank(BACKGROUND_LUMA);
    if placeholder == CameraPlaceholder::Connecting {
        draw_spinner(&mut frame, step);
    }
    if let Some(font) = font() {
        draw_text(&mut frame, font, text, TEXT_TOP);
    }
    frame
}

#[cfg(test)]
mod tests;
