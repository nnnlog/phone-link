import CoreGraphics

/// How a mirroring window is sized and placed. The window's video area keeps the video's aspect; the
/// window adds its title bar and toolbar around it, and the navigation bar under it while that shows
/// (`nonClient`, what the window adds to the video area each way).
///
/// Frames are in AppKit screen coordinates (origin at the bottom left, y upwards), in points; `visible`
/// is a screen's area without the menu bar and the Dock. Lengths are rounded to whole points, a half
/// to the even one.
///
/// - A new window without a saved frame fits, title bar and all, three quarters of the visible area's
///   width and height with the video's aspect, centred (`defaultFrame`). It then steps 32 points down
///   and right while another mirroring window on its screen has its top-left corner within 4 points of
///   its own, at most 8 times and only while it stays in the visible area (`cascaded`).
/// - A video of a new aspect reshapes the window (`reshaped`): not when the video area's aspect is within
///   0.01 of it; else the frame the window had before its last reshape returns when it had that aspect
///   and fits the visible area; else the video area takes the video's own size — a backing pixel per
///   video pixel — between the smallest window and three quarters of the largest window of that aspect
///   in the visible area. The window keeps its top-right corner where it then fits, else its top-left,
///   bottom-left or bottom-right corner, else its top-left corner moves into the visible area.
/// - The user resizes it with the aspect kept (`liveResized`), between `minimumSize` and the visible area.
public enum MirrorWindowSizing {
    /// The aspect (width ÷ height) of a window whose video and phone resolution are not known.
    public static let defaultAspect: CGFloat = 0.5125
    /// A new window's share of the visible area each way, and the largest share a reshape gives it.
    public static let defaultShare: CGFloat = 0.75
    /// Aspects closer than this are the same.
    public static let aspectTolerance: CGFloat = 0.01
    /// The smallest window: this wide for a portrait video (aspect at most 1)…
    public static let minimumWidth: CGFloat = 240
    /// …and this high for a landscape one.
    public static let minimumHeight: CGFloat = 200
    /// The navigation bar under the video.
    public static let navigationBarHeight: CGFloat = 32
    /// How far a new window steps from another one at its place, how many times, and how close their
    /// top-left corners are to count as the same place.
    public static let cascadeStep: CGFloat = 32
    public static let cascadeAttempts = 8
    public static let cascadeCollision: CGFloat = 4

    /// A screen as the window sees it: its whole frame, and the visible area inside it.
    public struct Screen: Equatable, Sendable {
        public var frame: CGRect
        public var visible: CGRect

        public init(frame: CGRect, visible: CGRect) {
            self.frame = frame
            self.visible = visible
        }
    }

    /// A window frame and the video aspect it was shaped for.
    public struct Placement: Equatable, Sendable {
        public var aspect: CGFloat
        public var frame: CGRect

        public init(aspect: CGFloat, frame: CGRect) {
            self.aspect = aspect
            self.frame = frame
        }
    }

    /// Which way the user drags the window's border: a side edge (the height follows the width), the top
    /// or bottom edge (the width follows the height), or a corner (the height follows the width for a
    /// landscape video, the width the height for a portrait one).
    public enum ResizeEdge: Equatable, Sendable {
        case horizontal
        case vertical
        case corner
    }

    // MARK: - A new window

    /// A new window's frame for a video of `aspect` (width ÷ height), centred in `visible`.
    public static func defaultFrame(aspect: CGFloat, visible: CGRect, nonClient: CGSize) -> CGRect {
        let box = CGSize(width: round(visible.width * defaultShare), height: round(visible.height * defaultShare))
        let room = CGSize(width: box.width - nonClient.width, height: box.height - nonClient.height)
        let roomAspect = room.height > 0 ? room.width / room.height : aspect

        let size = aspect <= roomAspect
            ? CGSize(width: round(room.height * aspect) + nonClient.width, height: box.height)
            : CGSize(width: box.width, height: round(room.width / aspect) + nonClient.height)

        let left = ((visible.width - size.width) / 2).rounded(.towardZero)
        let top = ((visible.height - size.height) / 2).rounded(.towardZero)
        return CGRect(x: visible.minX + left, y: visible.maxY - top - size.height, width: size.width, height: size.height)
    }

    /// `frame` stepped down and right away from the other mirroring windows (`peers`) on its screen:
    /// the first step whose top-left corner is not within `cascadeCollision` of a peer's, as long as the
    /// window stays in that screen's visible area; else `frame` itself.
    public static func cascaded(_ frame: CGRect, screens: [Screen], peers: [CGRect]) -> CGRect {
        let frames = screens.map(\.frame)
        guard let index = screenIndex(for: frame, among: frames) else { return frame }

        let onScreen = peers.filter { screenIndex(for: $0, among: frames) == index }
        guard !onScreen.isEmpty else { return frame }

        let visible = screens[index].visible
        for step in 0 ..< cascadeAttempts {
            let offset = CGFloat(step) * cascadeStep
            let candidate = frame.offsetBy(dx: offset, dy: -offset)
            guard fits(candidate, in: visible) else { break }

            let collides = onScreen.contains {
                abs($0.minX - candidate.minX) <= cascadeCollision && abs($0.maxY - candidate.maxY) <= cascadeCollision
            }
            if !collides { return candidate }
        }
        return frame
    }

    /// The screen `rect` is on: the one it overlaps most, else the nearest.
    public static func screenIndex(for rect: CGRect, among screens: [CGRect]) -> Int? {
        let areas = screens.map { screen -> CGFloat in
            let common = screen.intersection(rect)
            return common.isNull ? 0 : common.width * common.height
        }
        if let most = areas.indices.max(by: { areas[$0] < areas[$1] }), areas[most] > 0 {
            return most
        }

        func distance(_ screen: CGRect) -> CGFloat {
            let dx = max(0, screen.minX - rect.maxX, rect.minX - screen.maxX)
            let dy = max(0, screen.minY - rect.maxY, rect.minY - screen.maxY)
            return (dx * dx + dy * dy).squareRoot()
        }
        return screens.indices.min { distance(screens[$0]) < distance(screens[$1]) }
    }

    /// A saved frame to open with: as it was saved, when it overlaps a screen; `nil` when it overlaps
    /// none (or is empty).
    public static func restoredFrame(_ saved: CGRect, screens: [CGRect]) -> CGRect? {
        guard saved.width > 0, saved.height > 0 else { return nil }
        let overlaps = screens.contains { screen in
            let common = screen.intersection(saved)
            return !common.isNull && common.width > 0 && common.height > 0
        }
        return overlaps ? saved : nil
    }

    // MARK: - A new aspect

    /// The frame of a window now at `current` for a video of `content` points (a point per backing
    /// pixel's worth of video), or `nil` when the window's video area is already within 0.01 of the
    /// video's aspect. `fallbackAspect` stands for the window's when its video area has no height;
    /// `previous` is the frame the window had before its last reshape.
    public static func reshaped(
        content: CGSize,
        current: CGRect,
        fallbackAspect: CGFloat,
        previous: Placement?,
        visible: CGRect,
        nonClient: CGSize
    ) -> CGRect? {
        guard content.width > 0, content.height > 0 else { return nil }
        let aspect = content.width / content.height
        let currentAspect = contentAspect(current, nonClient: nonClient) ?? fallbackAspect
        guard !sameAspect(aspect, currentAspect) else { return nil }

        if let previous, sameAspect(previous.aspect, aspect), fits(previous.frame, in: visible) {
            return previous.frame
        }

        let size = reshapedSize(content: content, visible: visible, nonClient: nonClient)
        return CGRect(origin: anchored(size, current: current, visible: visible), size: size)
    }

    /// The window size for a video of `content`: its own size where that lies between the smallest
    /// window and three quarters of the largest one of its aspect, else the nearer of the two.
    static func reshapedSize(content: CGSize, visible: CGRect, nonClient: CGSize) -> CGSize {
        let aspect = content.width / content.height
        let minimum = minimumSize(aspect: aspect, visible: visible, nonClient: nonClient)
        let maximum = maximumSize(aspect: aspect, visible: visible, nonClient: nonClient)

        let low = CGSize(width: minimum.width - nonClient.width, height: minimum.height - nonClient.height)
        var high = CGSize(
            width: round(maximum.width * defaultShare) - nonClient.width,
            height: round(maximum.height * defaultShare) - nonClient.height
        )
        if high.width < low.width {
            high.width = max(low.width, maximum.width - nonClient.width)
        }
        if high.height < low.height {
            high.height = max(low.height, maximum.height - nonClient.height)
        }

        let lowest = max(low.width / content.width, low.height / content.height)
        let highest = min(high.width / content.width, high.height / content.height)
        let scale = lowest < highest ? min(max(1, lowest), highest) : highest

        let width = clamp(round(content.width * scale), low.width, high.width)
        let height = clamp(round(content.height * scale), low.height, high.height)
        return CGSize(width: width + nonClient.width, height: height + nonClient.height)
    }

    /// Where a window of `size` goes from `current`: the first of its top-right, top-left, bottom-left
    /// and bottom-right corners kept that leaves it inside `visible`, else its top-left corner moved
    /// into `visible`.
    static func anchored(_ size: CGSize, current: CGRect, visible: CGRect) -> CGPoint {
        let candidates = [
            CGPoint(x: current.maxX - size.width, y: current.maxY - size.height),
            CGPoint(x: current.minX, y: current.maxY - size.height),
            CGPoint(x: current.minX, y: current.minY),
            CGPoint(x: current.maxX - size.width, y: current.minY),
        ]
        for origin in candidates where fits(CGRect(origin: origin, size: size), in: visible) {
            return origin
        }

        let x = clamp(current.minX, visible.minX, max(visible.minX, visible.maxX - size.width))
        let drop = clamp(visible.maxY - current.maxY, 0, max(0, visible.height - size.height))
        return CGPoint(x: x, y: visible.maxY - drop - size.height)
    }

    // MARK: - Size limits

    /// The smallest window for a video of `aspect`: 240 points wide for a portrait video (the height
    /// from the aspect, at most the visible height), 200 points high for a landscape one (the width
    /// from the aspect, at least 240 points and at most the visible width).
    public static func minimumSize(aspect: CGFloat, visible: CGRect, nonClient: CGSize) -> CGSize {
        if aspect <= 1 {
            let height = round((minimumWidth - nonClient.width) / aspect) + nonClient.height
            return CGSize(width: minimumWidth, height: min(visible.height, height))
        }
        let width = round((minimumHeight - nonClient.height) * aspect) + nonClient.width
        return CGSize(width: min(visible.width, max(minimumWidth, width)), height: minimumHeight)
    }

    /// The largest window of `aspect` in `visible`: its width when the window is as high as the
    /// visible area, its height when it is as wide, each between the smallest window and the visible
    /// area.
    public static func maximumSize(aspect: CGFloat, visible: CGRect, nonClient: CGSize) -> CGSize {
        let minimum = minimumSize(aspect: aspect, visible: visible, nonClient: nonClient)
        let width = round((visible.height - nonClient.height) * aspect) + nonClient.width
        let height = round((visible.width - nonClient.width) / aspect) + nonClient.height
        return CGSize(
            width: clamp(width, minimum.width, visible.width),
            height: clamp(height, minimum.height, visible.height)
        )
    }

    // MARK: - The user resizing

    /// The edge the user drags, from the size AppKit proposes, the size the drag started from and the
    /// window's size now: a side counts as dragged when the proposal differs from both. Once a corner,
    /// the drag stays one; until both sides move, the side that moved decides.
    public static func resizeEdge(proposed: CGSize, start: CGSize, current: CGSize, known: ResizeEdge?) -> ResizeEdge? {
        if known == .corner { return .corner }

        let width = proposed.width != start.width && proposed.width != current.width
        let height = proposed.height != start.height && proposed.height != current.height
        if width, height { return .corner }
        if let known { return known }
        if width { return .horizontal }
        if height { return .vertical }
        return nil
    }

    /// The size the window takes for the user's `proposed` one: the side that follows recomputed from
    /// the aspect, then the height and width capped at the visible area's, the other side following.
    public static func liveResized(_ proposed: CGSize, edge: ResizeEdge?, aspect: CGFloat, visible: CGRect, nonClient: CGSize) -> CGSize {
        var size = proposed
        func heightFromWidth() {
            size.height = round((size.width - nonClient.width) / aspect) + nonClient.height
        }
        func widthFromHeight() {
            size.width = round((size.height - nonClient.height) * aspect) + nonClient.width
        }

        switch edge {
        case .horizontal?:
            heightFromWidth()
        case .vertical?:
            widthFromHeight()
        case .corner?:
            if aspect <= 1 { widthFromHeight() } else { heightFromWidth() }
        case nil:
            break
        }

        if size.height > visible.height {
            size.height = visible.height
            widthFromHeight()
        }
        if size.width > visible.width {
            size.width = visible.width
            heightFromWidth()
        }
        return size
    }

    /// The largest frame of `aspect` inside `visible`, centred: the zoomed window.
    public static func zoomedFrame(aspect: CGFloat, visible: CGRect, nonClient: CGSize) -> CGRect {
        let room = CGSize(width: visible.width - nonClient.width, height: visible.height - nonClient.height)
        var content = CGSize(width: room.width, height: (room.width / aspect).rounded(.down))
        if content.height > room.height {
            content = CGSize(width: (room.height * aspect).rounded(.down), height: room.height)
        }

        let size = CGSize(width: content.width + nonClient.width, height: content.height + nonClient.height)
        return CGRect(
            x: visible.minX + ((visible.width - size.width) / 2).rounded(.down),
            y: visible.minY + ((visible.height - size.height) / 2).rounded(.down),
            width: size.width,
            height: size.height
        )
    }

    // MARK: - The window's chrome changing

    /// Whether a window of `frame` already holds the navigation bar: its video area without the bar's
    /// height is within 0.01 of `aspect`, and nearer to it than with that height.
    public static func holdsNavigationBar(_ frame: CGRect, nonClientWithoutBar: CGSize, aspect: CGFloat) -> Bool {
        let width = frame.width - nonClientWithoutBar.width
        let height = frame.height - nonClientWithoutBar.height
        let without = height - navigationBarHeight
        guard width > 0, without > 0 else { return false }

        let apart = abs(width / without - aspect)
        return apart <= aspectTolerance && apart < abs(width / height - aspect)
    }

    /// `frame` made `delta` points higher (lower when negative), its top edge kept, its height between
    /// the smallest and the largest window of `aspect`.
    public static func heightChanged(_ frame: CGRect, by delta: CGFloat, aspect: CGFloat, visible: CGRect, nonClient: CGSize) -> CGRect {
        let minimum = minimumSize(aspect: aspect, visible: visible, nonClient: nonClient)
        let maximum = maximumSize(aspect: aspect, visible: visible, nonClient: nonClient)
        let height = clamp(frame.height + delta, minimum.height, maximum.height)
        return CGRect(x: frame.minX, y: frame.maxY - height, width: frame.width, height: height)
    }

    // MARK: - Helpers

    /// The aspect of `frame`'s video area, or `nil` when it has no height.
    public static func contentAspect(_ frame: CGRect, nonClient: CGSize) -> CGFloat? {
        let height = frame.height - nonClient.height
        guard height > 0 else { return nil }
        return (frame.width - nonClient.width) / height
    }

    public static func sameAspect(_ a: CGFloat, _ b: CGFloat) -> Bool {
        abs(a - b) <= aspectTolerance
    }

    /// `rect` lies inside `bounds`, edges included.
    static func fits(_ rect: CGRect, in bounds: CGRect) -> Bool {
        rect.minX >= bounds.minX && rect.maxX <= bounds.maxX && rect.minY >= bounds.minY && rect.maxY <= bounds.maxY
    }

    static func round(_ value: CGFloat) -> CGFloat {
        value.rounded(.toNearestOrEven)
    }

    /// `value` between `low` and `high`; `low` wins when they cross.
    static func clamp(_ value: CGFloat, _ low: CGFloat, _ high: CGFloat) -> CGFloat {
        max(low, min(value, high))
    }
}
