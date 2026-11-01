import CoreGraphics
import PhoneLinkEngine

/// How a mirroring window lays out the phone's video, and where a point of the window falls on the
/// phone's screen.
///
/// The phone reports its screen size in its current orientation (the device resolution), how many
/// degrees to turn the video to show it upright, and optionally the part of the screen that has
/// content (a folded phone's half). The whole screen as drawn has the aspect of the turned video
/// (of the device resolution the phone registered, which the window starts with, until a video size
/// is known; a later device resolution does not change it), and the video fills it. The window keeps
/// the content's aspect: it fits the content rectangle into its bounds, and draws the whole screen
/// scaled the same way around it, clipped. Input coordinates are normalized to that whole-screen
/// rectangle, which the engine scales to the device resolution. Rectangles here have their origin at
/// the top left, y downwards.
public struct MirrorDisplay: Equatable, Sendable {
    /// The phone's screen in its current orientation, in pixels.
    public var deviceSize: CGSize?
    /// The device resolution the window started with (the one the phone registered).
    public var initialDeviceSize: CGSize?
    /// The decoded video's size, before rotation.
    public var videoSize: CGSize?
    /// Degrees to turn the video: 0, −90, −180 or −270 (negative = counter-clockwise on screen).
    public var rotation: Int
    /// The content's rectangle inside the device resolution.
    public var contentArea: CGRect?

    public init(
        deviceSize: CGSize? = nil,
        initialDeviceSize: CGSize? = nil,
        videoSize: CGSize? = nil,
        rotation: Int = 0,
        contentArea: CGRect? = nil
    ) {
        self.deviceSize = deviceSize
        self.initialDeviceSize = initialDeviceSize
        self.videoSize = videoSize
        self.rotation = rotation
        self.contentArea = contentArea
    }

    public init(_ session: MirrorSessionView) {
        func size(_ s: MirrorSize?) -> CGSize? {
            guard let s, s.width > 0, s.height > 0 else { return nil }
            return CGSize(width: CGFloat(s.width), height: CGFloat(s.height))
        }

        let area = session.contentArea.map {
            CGRect(x: CGFloat($0.left), y: CGFloat($0.top), width: CGFloat($0.right - $0.left), height: CGFloat($0.bottom - $0.top))
        }

        self.init(
            deviceSize: size(session.deviceResolution),
            initialDeviceSize: size(session.initialDeviceResolution),
            videoSize: size(session.videoSize),
            rotation: Int(session.rotation),
            contentArea: area
        )
    }

    /// The rotation turns the picture on its side.
    public var swapsSides: Bool {
        (rotation / 90) % 2 != 0
    }

    /// The video's size once rotated.
    public var rotatedVideoSize: CGSize? {
        videoSize.map { swapsSides ? CGSize(width: $0.height, height: $0.width) : $0 }
    }

    /// The phone's whole screen as the window shows it: the turned video, else the device resolution
    /// the window started with.
    public var screenSize: CGSize? {
        rotatedVideoSize ?? initialDeviceSize
    }

    /// The part of the screen the window keeps in view: the content area (given inside the device
    /// resolution, scaled to the screen as drawn) when it lies inside the device resolution, else the
    /// whole screen.
    public var visibleArea: CGRect? {
        guard let screen = screenSize else { return nil }
        let whole = CGRect(origin: .zero, size: screen)
        if let area = contentArea, area.width > 0, area.height > 0, let device = deviceSize,
           CGRect(origin: .zero, size: device).contains(area) {
            let sx = screen.width / device.width
            let sy = screen.height / device.height
            return CGRect(x: area.minX * sx, y: area.minY * sy, width: area.width * sx, height: area.height * sy)
        }
        return whole
    }

    /// The size whose aspect the window keeps.
    public var aspectSize: CGSize? {
        visibleArea?.size
    }

    /// Where the whole screen goes in `bounds` (top-left origin): the visible area fitted into the
    /// bounds and centred, the rest of the screen around it at the same scale.
    public func screenRect(in bounds: CGSize) -> CGRect? {
        guard let screen = screenSize, let visible = visibleArea, bounds.width > 0, bounds.height > 0 else { return nil }
        let scale = min(bounds.width / visible.width, bounds.height / visible.height)
        let shown = CGSize(width: visible.width * scale, height: visible.height * scale)
        let origin = CGPoint(x: (bounds.width - shown.width) / 2, y: (bounds.height - shown.height) / 2)

        return CGRect(
            x: origin.x - visible.minX * scale,
            y: origin.y - visible.minY * scale,
            width: screen.width * scale,
            height: screen.height * scale
        )
    }

    /// `point` (top-left origin, in `bounds`) normalized to the screen rectangle: 0–1 inside it.
    public func normalized(_ point: CGPoint, in bounds: CGSize) -> CGPoint? {
        guard let rect = screenRect(in: bounds), rect.width > 0, rect.height > 0 else { return nil }
        return CGPoint(x: (point.x - rect.minX) / rect.width, y: (point.y - rect.minY) / rect.height)
    }

    /// The angle to turn the video layer by, in radians, in a layer whose y axis points up (where a
    /// positive angle turns counter-clockwise).
    public var layerRotation: CGFloat {
        CGFloat(-rotation) * .pi / 180
    }
}

/// Turns macOS scroll events into the phone's wheel units: ±120 per notch (positive = away from the
/// user, or to the right), one scroll per macOS event. A mouse wheel reports lines, one line a notch; precise scrolling (a trackpad, a Magic
/// Mouse) reports points, taken as lines of the system's points per line. What falls short of a whole
/// unit carries over to the next event until the gesture ends.
public struct MirrorScrollAccumulator: Sendable {
    public static let notch: Double = 120

    private var x: Double = 0
    private var y: Double = 0

    public init() {}

    /// The wheel deltas (x, y) for one macOS scroll of `deltaX`, `deltaY` (AppKit's scrolling deltas:
    /// positive y moves the content down, i.e. scrolls up; positive x moves it right), or `nil` when it
    /// adds up to less than a unit. `pointsPerLine` converts precise deltas to lines.
    public mutating func add(deltaX: Double, deltaY: Double, precise: Bool, pointsPerLine: Double) -> (x: Int32, y: Int32)? {
        let line = precise ? max(pointsPerLine, 1) : 1
        // AppKit's positive x reveals what is on the left; the phone's positive x scrolls right.
        x += -deltaX / line * Self.notch
        y += deltaY / line * Self.notch

        let whole = (x: x.rounded(.towardZero), y: y.rounded(.towardZero))
        x -= whole.x
        y -= whole.y
        guard whole.x != 0 || whole.y != 0 else { return nil }

        func units(_ v: Double) -> Int32 {
            Int32(min(max(v, Double(Int32.min)), Double(Int32.max)))
        }
        return (units(whole.x), units(whole.y))
    }

    /// Forgets what fell short of a unit (a new gesture starts).
    public mutating func reset() {
        x = 0
        y = 0
    }
}

/// The Mac's modifier keys as the phone receives them: which way a modifier key went, read from a
/// `flagsChanged` event's key code and its (device-dependent) modifier flags.
public enum MirrorModifierKeys {
    /// What a modifier key's change sends.
    public enum Change: Equatable, Sendable {
        case down
        case up
        /// Caps Lock toggled: a press and a release.
        case toggle
    }

    /// The device-dependent flag bit of each modifier key (`NX_DEVICE…KEYMASK`), by `kVK_*` code.
    static let deviceMasks: [UInt16: UInt] = [
        0x38: 0x0000_0002, // left Shift
        0x3C: 0x0000_0004, // right Shift
        0x3B: 0x0000_0001, // left Control
        0x3E: 0x0000_2000, // right Control
        0x3A: 0x0000_0020, // left Option
        0x3D: 0x0000_0040, // right Option
        0x37: 0x0000_0008, // left Command
        0x36: 0x0000_0010, // right Command
    ]

    /// The device-independent flag of each modifier key, used when a keyboard reports no
    /// device-dependent bits.
    static let independentMasks: [UInt16: UInt] = [
        0x38: 1 << 17, 0x3C: 1 << 17, // Shift
        0x3B: 1 << 18, 0x3E: 1 << 18, // Control
        0x3A: 1 << 19, 0x3D: 1 << 19, // Option
        0x37: 1 << 20, 0x36: 1 << 20, // Command
        0x3F: 1 << 23, // Function
    ]

    public static let capsLock: UInt16 = 0x39

    /// The change of `keyCode` given the flags after it (`NSEvent.modifierFlags.rawValue`); `nil` for
    /// a key code that is no modifier key.
    public static func change(keyCode: UInt16, flags: UInt) -> Change? {
        if keyCode == capsLock { return .toggle }
        if let mask = deviceMasks[keyCode], flags & 0xFFFF != 0 {
            return flags & mask != 0 ? .down : .up
        }
        if let mask = independentMasks[keyCode] {
            return flags & mask != 0 ? .down : .up
        }
        return nil
    }
}

/// The length of an Opus packet in samples, from its table-of-contents byte (RFC 6716 § 3.1).
public enum OpusPacket {
    /// Samples per channel at `sampleRate`; `nil` for an empty or malformed packet.
    public static func sampleCount(_ packet: [UInt8], sampleRate: Int) -> Int? {
        guard let toc = packet.first else { return nil }
        let config = Int(toc >> 3)

        // Frame length in units of 2.5 ms.
        let frameQuarterMs: Int
        if config < 12 {
            frameQuarterMs = [4, 8, 16, 24][config & 3]
        } else if config < 16 {
            frameQuarterMs = [4, 8][config & 1]
        } else {
            frameQuarterMs = [1, 2, 4, 8][config & 3]
        }

        let frames: Int
        switch toc & 3 {
        case 0: frames = 1
        case 1, 2: frames = 2
        default:
            guard packet.count >= 2 else { return nil }
            frames = Int(packet[1] & 0x3F)
        }
        guard frames > 0 else { return nil }

        let samples = frames * frameQuarterMs * sampleRate / 400
        // A packet lasts at most 120 ms.
        guard samples <= sampleRate * 120 / 1000 else { return nil }
        return samples
    }
}
