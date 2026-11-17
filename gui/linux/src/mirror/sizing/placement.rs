//! Where a window opens: a kept frame on a screen, stepped away from the other mirroring windows.

use super::geometry::{Rect, Screen};

/// How far a window steps from another one at its place, how many times, and how close their top-left
/// corners are to count as the same place.
const CASCADE_STEP: f64 = 32.0;
const CASCADE_ATTEMPTS: usize = 8;
const CASCADE_COLLISION: f64 = 4.0;

/// A kept frame to open with: as it was kept, when it overlaps a screen.
///
/// It is `None` when it overlaps none (or is empty).
pub(super) fn restored(saved: Rect, screens: &[Rect]) -> Option<Rect> {
    if saved.width <= 0.0 || saved.height <= 0.0 {
        return None;
    }
    screens.iter().any(|screen| screen.common_area(&saved) > 0.0).then_some(saved)
}

/// The screen `rect` is on: the one it overlaps most, else the nearest.
pub(super) fn screen_index(rect: &Rect, screens: &[Rect]) -> Option<usize> {
    let areas: Vec<f64> = screens.iter().map(|screen| screen.common_area(rect)).collect();
    let most = areas.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1));
    if let Some((index, area)) = most
        && *area > 0.0
    {
        return Some(index);
    }

    let distance = |screen: &Rect| {
        let dx = (screen.x - rect.right()).max(rect.x - screen.right()).max(0.0);
        let dy = (screen.y - rect.bottom()).max(rect.y - screen.bottom()).max(0.0);
        (dx * dx + dy * dy).sqrt()
    };
    screens.iter().enumerate().min_by(|a, b| distance(a.1).total_cmp(&distance(b.1))).map(|(index, _)| index)
}

/// `frame` stepped down and right away from the other mirroring windows (`peers`) on its screen.
///
/// It takes the first step whose top-left corner is not within 4 pixels of a peer's, as long as the
/// window stays in that screen's visible room; else `frame` itself.
pub(super) fn cascaded(frame: Rect, screens: &[Screen], peers: &[Rect]) -> Rect {
    let frames: Vec<Rect> = screens.iter().map(|screen| screen.frame).collect();
    let Some(index) = screen_index(&frame, &frames) else { return frame };

    let on_screen: Vec<&Rect> = peers.iter().filter(|peer| screen_index(peer, &frames) == Some(index)).collect();
    if on_screen.is_empty() {
        return frame;
    }

    let visible = screens[index].visible;
    for step in 0..CASCADE_ATTEMPTS {
        let candidate = frame.offset(step as f64 * CASCADE_STEP);
        if !candidate.fits_in(&visible) {
            break;
        }
        let collides = on_screen.iter().any(|peer| {
            (peer.x - candidate.x).abs() <= CASCADE_COLLISION && (peer.y - candidate.y).abs() <= CASCADE_COLLISION
        });
        if !collides {
            return candidate;
        }
    }
    frame
}
