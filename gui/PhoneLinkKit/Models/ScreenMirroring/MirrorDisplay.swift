import CoreGraphics
import PhoneLinkEngine

/// How a mirroring window lays out the phone's video, and where a point of the window falls on the
/// phone's screen.
///
/// The phone reports its screen size in its current orientation (the device resolution) and how many
/// degrees to turn the video to show it upright. The turned video is scaled the same way both ways to
/// fit the window's video area and centred in it, bars filling the rest; input positions are normalized
/// to the video as drawn, which the engine scales to the device resolution. The part of the screen the
/// phone says has content (a folded phone's half) is not used: the whole video shows. Rectangles here
/// have their origin at the top left, y downwards.
public struct MirrorDisplay: Equatable, Sendable {
    /// The phone's screen in its current orientation, in pixels.
    public var deviceSize: CGSize?
    /// The phone record's device resolution the window started with.
    public var initialDeviceSize: CGSize?
    /// The decoded video's size, before rotation.
    public var videoSize: CGSize?
    /// Degrees to turn the video: 0, −90, −180 or −270 (negative = counter-clockwise on screen).
    public var rotation: Int

    /// What the video is drawn with: its decoded size and its rotation. A window fades the video out
    /// and in around a change of it (`MirrorVideoTransitions`).
    public struct Transform: Equatable, Sendable {
        public var videoSize: CGSize
        public var rotation: Int
    }

    public init(
        deviceSize: CGSize? = nil,
        initialDeviceSize: CGSize? = nil,
        videoSize: CGSize? = nil,
        rotation: Int = 0
    ) {
        self.deviceSize = deviceSize
        self.initialDeviceSize = initialDeviceSize
        self.videoSize = videoSize
        self.rotation = rotation
    }

    public init(_ session: MirrorSessionView) {
        func size(_ s: MirrorSize?) -> CGSize? {
            guard let s, s.width > 0, s.height > 0 else { return nil }
            return CGSize(width: CGFloat(s.width), height: CGFloat(s.height))
        }

        self.init(
            deviceSize: size(session.deviceResolution),
            initialDeviceSize: size(session.initialDeviceResolution),
            videoSize: size(session.videoSize),
            rotation: Int(session.rotation)
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

    /// The phone's screen as the window shows it: the turned video; nothing before its size is known.
    public var screenSize: CGSize? {
        rotatedVideoSize
    }

    /// The size whose aspect a new window opens with: the turned video, else the phone record's device
    /// resolution.
    public var openingSize: CGSize? {
        rotatedVideoSize ?? initialDeviceSize
    }

    /// The size whose aspect the window's video area has once the phone sent its screen: the turned
    /// video, else the current device resolution.
    public var shownSize: CGSize? {
        rotatedVideoSize ?? deviceSize
    }

    /// The video's transform; `nil` before its size is known.
    public var transform: Transform? {
        videoSize.map { Transform(videoSize: $0, rotation: rotation) }
    }

    /// Where the turned video goes in `bounds` (top-left origin): scaled the same way both ways to fit
    /// and centred.
    public func screenRect(in bounds: CGSize) -> CGRect? {
        guard let screen = screenSize, bounds.width > 0, bounds.height > 0 else { return nil }
        let scale = min(bounds.width / screen.width, bounds.height / screen.height)
        let shown = CGSize(width: screen.width * scale, height: screen.height * scale)

        return CGRect(
            x: (bounds.width - shown.width) / 2,
            y: (bounds.height - shown.height) / 2,
            width: shown.width,
            height: shown.height
        )
    }

    /// `point` (top-left origin, in `bounds`) normalized to the video as drawn: 0–1 inside it.
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

/// When a window applies a new video transform: the first at once; a different one after fading the
/// video out for `fadeLength`, keeping it hidden for `hiddenLength`, then fading it in for `fadeLength`.
/// A transform is compared with the last one received, applied yet or not.
public struct MirrorVideoTransitions: Sendable {
    public enum Step: Equatable, Sendable {
        /// Show the display at once.
        case apply
        /// Fade the video out, show the display, fade it in.
        case fade
    }

    public static let fadeLength: Duration = .milliseconds(125)
    public static let hiddenLength: Duration = .milliseconds(400)
    /// `fadeLength` in seconds, for an animation.
    public static let fadeSeconds: Double = 0.125

    private var last: MirrorDisplay.Transform?

    public init() {}

    /// How to show a display of `transform`.
    public mutating func receive(_ transform: MirrorDisplay.Transform?) -> Step {
        guard let transform else { return .apply }
        defer { last = transform }

        guard let last, last != transform else { return .apply }
        return .fade
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
