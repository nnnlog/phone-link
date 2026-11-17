//! The sizes, rectangles and screens a window is sized and placed in, and how lengths are rounded.

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Size {
    pub(super) width: f64,
    pub(super) height: f64,
}

impl Size {
    pub(super) const fn new(width: f64, height: f64) -> Self {
        Self { width, height }
    }
}

/// A rectangle with its origin at the top left, y downwards.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Rect {
    pub(super) x: f64,
    pub(super) y: f64,
    pub(super) width: f64,
    pub(super) height: f64,
}

impl Rect {
    pub(super) const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self { x, y, width, height }
    }

    pub(super) fn right(&self) -> f64 {
        self.x + self.width
    }

    pub(super) fn bottom(&self) -> f64 {
        self.y + self.height
    }

    pub(super) fn offset(&self, by: f64) -> Self {
        Self { x: self.x + by, y: self.y + by, ..*self }
    }

    /// The area this rectangle shares with `other`; 0 when they only touch or are apart.
    pub(super) fn common_area(&self, other: &Rect) -> f64 {
        let width = self.right().min(other.right()) - self.x.max(other.x);
        let height = self.bottom().min(other.bottom()) - self.y.max(other.y);
        if width > 0.0 && height > 0.0 { width * height } else { 0.0 }
    }

    /// `self` lies inside `bounds`, edges included.
    pub(super) fn fits_in(&self, bounds: &Rect) -> bool {
        self.x >= bounds.x && self.right() <= bounds.right() && self.y >= bounds.y && self.bottom() <= bounds.bottom()
    }
}

/// A screen as a window sees it: its whole frame, and the room for windows inside it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Screen {
    pub(super) frame: Rect,
    pub(super) visible: Rect,
}

/// A window size and the video aspect it was shaped for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Placement {
    pub(super) aspect: f64,
    pub(super) size: Size,
}

/// `value` rounded to a whole pixel, a half to the even one.
pub(super) fn round(value: f64) -> f64 {
    value.round_ties_even()
}

/// `value` between `low` and `high`; `low` wins when they cross.
pub(super) fn clamp(value: f64, low: f64, high: f64) -> f64 {
    low.max(value.min(high))
}
