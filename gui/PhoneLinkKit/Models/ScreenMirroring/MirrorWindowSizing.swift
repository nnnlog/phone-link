import CoreGraphics

/// How a mirroring window is sized and placed: its content always has the phone video's aspect, so
/// the video fills it.
///
/// Frames are in AppKit screen coordinates (origin at the bottom left, y upwards); `chrome` is what
/// the window adds around its content (its title bar and toolbar), and `visible` the screen's area
/// without the menu bar and the Dock.
///
/// - A new window's content fits 75 % of the visible area's width and height, centred in it.
/// - When the aspect changes (the phone turns, or the video's size changes), a frame the window had
///   with that aspect is taken again if the window still shares a corner with it (turning back
///   restores the size). Otherwise the content keeps its height when it gets wider and its width when
///   it gets narrower, grows to the minimum window size, shrinks to 75 % of the visible area, and the
///   window keeps its top-right corner where it fits — else its top-left, bottom-left or bottom-right
///   corner — else moves into the visible area.
public enum MirrorWindowSizing {
    /// The aspect (width / height) of a window whose video size is not known yet.
    public static let defaultAspect: CGFloat = 0.5125
    /// The smallest window, in points.
    public static let minimumWindow = CGSize(width: 280, height: 240)
    /// A new window's content at most this share of the visible area, each way.
    public static let defaultShare: CGFloat = 0.75
    /// Aspects closer than this are the same.
    public static let aspectTolerance: CGFloat = 0.01
    /// Corners closer than this (points) are the same.
    public static let cornerTolerance: CGFloat = 2

    /// A new window's frame for content of `aspect`.
    public static func initialFrame(aspect: CGFloat, visible: CGRect, chrome: CGSize) -> CGRect {
        let content = clamped(fitted(aspect: aspect, visible: visible), aspect: aspect, visible: visible, chrome: chrome)
        let size = CGSize(width: content.width + chrome.width, height: content.height + chrome.height)
        return CGRect(
            x: visible.minX + (visible.width - size.width) / 2,
            y: visible.minY + (visible.height - size.height) / 2,
            width: size.width,
            height: size.height
        )
    }

    /// The frame for content of `aspect` of a window now at `current`; `earlier` are frames it had
    /// before, the latest last.
    public static func resizedFrame(aspect: CGFloat, current: CGRect, earlier: [CGRect], visible: CGRect, chrome: CGSize) -> CGRect {
        for frame in earlier.reversed() where sameAspect(contentAspect(frame, chrome: chrome), aspect) && sharesCorner(frame, current) {
            return frame
        }

        let currentContent = CGSize(width: current.width - chrome.width, height: current.height - chrome.height)
        var content: CGSize
        if currentContent.width > 0, currentContent.height > 0 {
            if currentContent.width / currentContent.height < aspect {
                content = CGSize(width: currentContent.height * aspect, height: currentContent.height)
            } else {
                content = CGSize(width: currentContent.width, height: currentContent.width / aspect)
            }
        } else {
            content = fitted(aspect: aspect, visible: visible)
        }

        content = clamped(content, aspect: aspect, visible: visible, chrome: chrome)
        let size = CGSize(width: content.width + chrome.width, height: content.height + chrome.height)
        return anchored(size, current: current, visible: visible)
    }

    /// The smallest content of `aspect`: a window at least 280 points wide when the content is narrower
    /// than the visible area (width ÷ height), at least 240 points high otherwise.
    public static func minimumContent(aspect: CGFloat, visible: CGRect, chrome: CGSize) -> CGSize {
        if visible.height <= 0 || aspect >= visible.width / visible.height {
            let height = max(0, minimumWindow.height - chrome.height)
            return CGSize(width: height * aspect, height: height)
        }
        let width = max(0, minimumWindow.width - chrome.width)
        return CGSize(width: width, height: width / aspect)
    }

    /// The largest frame of `aspect` inside `visible`, centred: the zoomed window.
    public static func zoomedFrame(aspect: CGFloat, visible: CGRect, chrome: CGSize) -> CGRect {
        let room = CGSize(width: visible.width - chrome.width, height: visible.height - chrome.height)
        var content = CGSize(width: room.width, height: room.width / aspect)
        if content.height > room.height {
            content = CGSize(width: room.height * aspect, height: room.height)
        }

        let size = CGSize(width: content.width + chrome.width, height: content.height + chrome.height)
        return CGRect(
            x: visible.minX + (visible.width - size.width) / 2,
            y: visible.minY + (visible.height - size.height) / 2,
            width: size.width,
            height: size.height
        )
    }

    /// A screen as the window sees it: its whole frame, and the visible area inside it.
    public struct Screen: Equatable, Sendable {
        public var frame: CGRect
        public var visible: CGRect

        public init(frame: CGRect, visible: CGRect) {
            self.frame = frame
            self.visible = visible
        }
    }

    /// The saved frame to use again, or `nil` for a new window's frame. A frame on no screen is not
    /// used; one on a screen is moved into the visible area of the screen it overlaps most — shrunk
    /// to it when larger — and then used only while no device resolution is known (`deviceAspect`,
    /// width ÷ height) or its content's aspect matches it. `rightToLeft` (a right-to-left layout)
    /// moves the right edge in before the left one.
    public static func restoredFrame(
        _ saved: CGRect,
        screens: [Screen],
        chrome: CGSize,
        deviceAspect: CGFloat?,
        rightToLeft: Bool = false
    ) -> CGRect? {
        guard saved.width > 0, saved.height > 0 else { return nil }

        let overlapping = screens.compactMap { screen -> (Screen, CGFloat)? in
            let common = screen.frame.intersection(saved)
            guard !common.isNull, common.width > 0, common.height > 0 else { return nil }
            return (screen, common.width * common.height)
        }
        guard let screen = overlapping.max(by: { $0.1 < $1.1 })?.0 else { return nil }

        let frame = bounded(saved, in: screen.visible, rightToLeft: rightToLeft)
        if let deviceAspect, !sameAspect(contentAspect(frame, chrome: chrome), deviceAspect) {
            return nil
        }
        return frame
    }

    // MARK: - Steps

    /// `rect` moved into `bounds` edge by edge, keeping its size where it fits: the bottom edge first,
    /// then the left edge (also when it is wider than `bounds`) and the right edge — the other way
    /// round for a right-to-left layout — then the top edge.
    private static func bounded(_ rect: CGRect, in bounds: CGRect, rightToLeft: Bool) -> CGRect {
        var (minX, maxX, minY, maxY) = (rect.minX, rect.maxX, rect.minY, rect.maxY)
        if minY < bounds.minY {
            minY = bounds.minY
            maxY = min(bounds.maxY, minY + rect.height)
        }

        if !rightToLeft {
            if minX < bounds.minX || rect.width > bounds.width {
                minX = bounds.minX
                maxX = min(bounds.maxX, minX + rect.width)
            }
            if maxX > bounds.maxX {
                maxX = bounds.maxX
                minX = max(bounds.minX, maxX - rect.width)
            }
        } else {
            if maxX > bounds.maxX {
                maxX = bounds.maxX
                minX = max(bounds.minX, maxX - rect.width)
            }
            if minX < bounds.minX {
                minX = bounds.minX
                maxX = min(bounds.maxX, minX + rect.width)
            }
        }

        if maxY > bounds.maxY {
            maxY = bounds.maxY
            minY = max(bounds.minY, maxY - rect.height)
        }

        return CGRect(x: minX, y: minY, width: maxX - minX, height: maxY - minY)
    }

    /// The default content: 75 % of the visible width, or of its height when that is the tighter one.
    private static func fitted(aspect: CGFloat, visible: CGRect) -> CGSize {
        let width = visible.width * defaultShare
        let height = visible.height * defaultShare
        if width / aspect <= height {
            return CGSize(width: width, height: width / aspect)
        }
        return CGSize(width: height * aspect, height: height)
    }

    /// `content` grown to the minimum window, then shrunk to 75 % of the visible area.
    private static func clamped(_ content: CGSize, aspect: CGFloat, visible: CGRect, chrome: CGSize) -> CGSize {
        let maxWidth = visible.width * defaultShare
        let maxHeight = visible.height * defaultShare
        let minWidth = max(0, minimumWindow.width - chrome.width)
        let minHeight = max(0, minimumWindow.height - chrome.height)

        var c = content
        if aspect < maxWidth / maxHeight, c.width < minWidth {
            c = CGSize(width: minWidth, height: minWidth / aspect)
        } else if aspect >= maxWidth / maxHeight, c.height < minHeight {
            c = CGSize(width: minHeight * aspect, height: minHeight)
        }

        if c.width > maxWidth {
            c = CGSize(width: maxWidth, height: maxWidth / aspect)
        }
        if c.height > maxHeight {
            c = CGSize(width: maxHeight * aspect, height: maxHeight)
        }

        return CGSize(width: c.width.rounded(), height: c.height.rounded())
    }

    /// `size` placed at the first corner of `current` that keeps it inside `visible`, else moved into it.
    private static func anchored(_ size: CGSize, current: CGRect, visible: CGRect) -> CGRect {
        let candidates = [
            CGPoint(x: current.maxX - size.width, y: current.maxY - size.height), // top right
            CGPoint(x: current.minX, y: current.maxY - size.height), // top left
            CGPoint(x: current.minX, y: current.minY), // bottom left
            CGPoint(x: current.maxX - size.width, y: current.minY), // bottom right
        ]

        for origin in candidates {
            let frame = CGRect(origin: origin, size: size)
            if visible.contains(frame) { return frame }
        }

        let last = candidates[3]
        return CGRect(
            x: max(visible.minX, min(visible.maxX - size.width, last.x)),
            y: max(visible.minY, min(visible.maxY - size.height, last.y)),
            width: size.width,
            height: size.height
        )
    }

    private static func contentAspect(_ frame: CGRect, chrome: CGSize) -> CGFloat {
        let height = frame.height - chrome.height
        return height > 0 ? (frame.width - chrome.width) / height : 0
    }

    private static func sameAspect(_ a: CGFloat, _ b: CGFloat) -> Bool {
        abs(a - b) <= aspectTolerance
    }

    private static func sharesCorner(_ a: CGRect, _ b: CGRect) -> Bool {
        func near(_ p: CGPoint, _ q: CGPoint) -> Bool {
            abs(p.x - q.x) < cornerTolerance && abs(p.y - q.y) < cornerTolerance
        }
        let corners = { (r: CGRect) in
            [CGPoint(x: r.minX, y: r.maxY), CGPoint(x: r.maxX, y: r.maxY), CGPoint(x: r.minX, y: r.minY), CGPoint(x: r.maxX, y: r.minY)]
        }
        return zip(corners(a), corners(b)).contains { near($0, $1) }
    }
}
