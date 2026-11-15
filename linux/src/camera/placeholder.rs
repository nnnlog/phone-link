use std::process::Command;
use std::sync::OnceLock;

use ab_glyph::{Font, FontVec, PxScale, ScaleFont, point};
use phonelink_connected_camera::CameraPlaceholder;

use super::{FRAME_BYTES, HEIGHT, WIDTH};

pub const CONNECTING_FRAMES: usize = 16;

const BACKGROUND_LUMA: u8 = 40;
const TEXT_LUMA: f32 = 235.0;
const TEXT_SIZE: f32 = 40.0;
const SPINNER_DOTS: usize = 8;

fn font() -> Option<&'static FontVec> {
    static FONT: OnceLock<Option<FontVec>> = OnceLock::new();
    FONT.get_or_init(|| {
        let output = Command::new("fc-match").args(["-f", "%{file}\n%{index}", "sans-serif:lang=ko"]).output().ok()?;
        let text = String::from_utf8(output.stdout).ok()?;
        let mut lines = text.lines();
        let path = lines.next()?;
        let index = lines.next().and_then(|i| i.parse().ok()).unwrap_or(0);
        let data = std::fs::read(path).ok()?;
        FontVec::try_from_vec_and_index(data, index).inspect_err(|e| eprintln!("{path} is no font: {e}")).ok()
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

fn draw_text(frame: &mut [u8], font: &FontVec, text: &str, baseline: f32) {
    let width_of = |size: f32| {
        let scaled = font.as_scaled(PxScale::from(size));
        text.chars().map(|c| scaled.h_advance(scaled.glyph_id(c))).sum::<f32>()
    };
    let limit = WIDTH as f32 - 160.0;
    let size = if width_of(TEXT_SIZE) > limit { TEXT_SIZE * limit / width_of(TEXT_SIZE) } else { TEXT_SIZE };
    let scaled = font.as_scaled(PxScale::from(size));
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

fn draw_spinner(frame: &mut [u8], step: usize) {
    let (center_x, center_y) = (WIDTH as f64 / 2.0, HEIGHT as f64 / 2.0 - 40.0);
    for dot in 0..SPINNER_DOTS {
        let angle = dot as f64 / SPINNER_DOTS as f64 * std::f64::consts::TAU;
        let (dx, dy) = (center_x + angle.sin() * 36.0, center_y - angle.cos() * 36.0);
        let lit = (dot + SPINNER_DOTS - step % SPINNER_DOTS) % SPINNER_DOTS;
        let strength = 1.0 - lit as f32 / SPINNER_DOTS as f32 * 0.8;
        for y in -8i64..=8 {
            for x in -8i64..=8 {
                if x * x + y * y <= 64 {
                    blend(frame, dx as i64 + x, dy as i64 + y, strength);
                }
            }
        }
    }
}

pub fn frame(placeholder: CameraPlaceholder, step: usize) -> Vec<u8> {
    let Some(text) = text(placeholder) else { return blank(16) };
    let mut frame = blank(BACKGROUND_LUMA);
    if placeholder == CameraPlaceholder::Connecting {
        draw_spinner(&mut frame, step);
    }
    if let Some(font) = font() {
        draw_text(&mut frame, font, text, HEIGHT as f32 / 2.0 + 60.0);
    }
    frame
}
