//! The placeholder images' unit tests: the spinner's ticks and video black.

use phonelink_connected_camera::CameraPlaceholder;

use super::{BACKGROUND_LUMA, CONNECTING_FRAMES, TEXT_LUMA, blank, draw_spinner, frame, tick_ends, tick_strength};
use crate::camera::frame::{FRAME_BYTES, HEIGHT, WIDTH};

/// The luma at the middle of the spinner's tick `tick`, and halfway to the next one.
fn tick_and_gap(frame: &[u8], tick: usize) -> (u8, u8) {
    let ((inner_x, inner_y), (outer_x, outer_y)) = tick_ends(tick);
    let middle = ((inner_x + outer_x) / 2.0, (inner_y + outer_y) / 2.0);
    let ((next_inner_x, next_inner_y), (next_outer_x, next_outer_y)) = tick_ends(tick + 1);
    let next = ((next_inner_x + next_outer_x) / 2.0, (next_inner_y + next_outer_y) / 2.0);
    let gap = ((middle.0 + next.0) / 2.0, (middle.1 + next.1) / 2.0);
    let at = |(x, y): (f64, f64)| frame[y as usize * WIDTH + x as usize];
    (at(middle), at(gap))
}

#[test]
fn the_spinner_has_a_tick_per_image() {
    for step in [0, 5, 15] {
        let mut frame = blank(BACKGROUND_LUMA);
        draw_spinner(&mut frame, step);
        let lumas: Vec<(u8, u8)> = (0..CONNECTING_FRAMES).map(|tick| tick_and_gap(&frame, tick)).collect();
        for (tick, (luma, gap)) in lumas.iter().enumerate() {
            assert!(*luma > BACKGROUND_LUMA + 30, "tick {tick} of step {step} shows: {luma}");
            assert_eq!(*gap, BACKGROUND_LUMA, "the ticks are apart");
        }
        assert_eq!(lumas[step].0, TEXT_LUMA as u8, "the tick at the step is the brightest");
        let after = lumas[(step + 1) % CONNECTING_FRAMES].0;
        assert!(after < lumas[(step + 15) % CONNECTING_FRAMES].0, "the oldest tick is the dimmest");
    }
}

#[test]
fn the_ticks_fade_behind_the_step() {
    let close = |a: f64, b: f64| (a - b).abs() < 1e-9;
    assert!(close(tick_strength(3, 3), 1.0));
    assert!(close(tick_strength(2, 3), 0.94));
    assert!(close(tick_strength(4, 3), 0.2), "the one ahead is the oldest");
    assert!(close(tick_strength(0, 16), 1.0), "the steps repeat");
}

#[test]
fn black_is_video_black() {
    let black = frame(CameraPlaceholder::Black, 0);
    assert_eq!(black.len(), FRAME_BYTES);
    assert!(black[..WIDTH * HEIGHT].iter().all(|&luma| luma == 16));
    assert!(black[WIDTH * HEIGHT..].iter().all(|&chroma| chroma == 128));
}
