import CoreGraphics
import Foundation
import Testing
@testable import PhoneLinkKit

struct MirrorWindowSizingTests {
    let visible = CGRect(x: 0, y: 0, width: 1600, height: 1000)
    let screen = MirrorWindowSizing.Screen(
        frame: CGRect(x: 0, y: 0, width: 1600, height: 1040),
        visible: CGRect(x: 0, y: 0, width: 1600, height: 1000)
    )
    /// A title bar and toolbar of 52 points.
    let nonClient = CGSize(width: 0, height: 52)

    /// The whole window, title bar included, fits 75 % of the visible area each way, centred.
    @Test func aNewWindowFitsThreeQuartersOfTheScreenTitleBarIncluded() {
        let portrait = MirrorWindowSizing.defaultFrame(aspect: 0.45, visible: visible, nonClient: nonClient)
        #expect(portrait == CGRect(x: 643, y: 125, width: 314, height: 750), "750 high in all, 698 of video")

        let wide = MirrorWindowSizing.defaultFrame(aspect: 16.0 / 9.0, visible: visible, nonClient: nonClient)
        #expect(wide == CGRect(x: 200, y: 137, width: 1200, height: 727), "1200 wide, 675 of video")
    }

    @Test func aNewWindowStepsAwayFromAnotherAtItsPlace() {
        let frame = CGRect(x: 643, y: 125, width: 314, height: 750)
        #expect(MirrorWindowSizing.cascaded(frame, screens: [screen], peers: []) == frame)

        // A peer at the same top-left corner: one step down and right.
        #expect(MirrorWindowSizing.cascaded(frame, screens: [screen], peers: [frame]) == frame.offsetBy(dx: 32, dy: -32))
        // Within 4 points counts; 5 points does not.
        let near = CGRect(x: 646, y: 128, width: 300, height: 744)
        #expect(MirrorWindowSizing.cascaded(frame, screens: [screen], peers: [near]) == frame.offsetBy(dx: 32, dy: -32))
        let apart = CGRect(x: 648, y: 125, width: 314, height: 750)
        #expect(MirrorWindowSizing.cascaded(frame, screens: [screen], peers: [apart]) == frame)

        // Two peers in a row: two steps.
        let peers = [frame, frame.offsetBy(dx: 32, dy: -32)]
        #expect(MirrorWindowSizing.cascaded(frame, screens: [screen], peers: peers) == frame.offsetBy(dx: 64, dy: -64))

        // A step that leaves the visible area ends the cascade where it started.
        let low = CGRect(x: 643, y: 10, width: 314, height: 750)
        #expect(MirrorWindowSizing.cascaded(low, screens: [screen], peers: [low]) == low)

        // At most 8 places are tried: with all 8 taken the window stays where it started.
        let small = CGRect(x: 100, y: 500, width: 200, height: 300)
        let crowded = (0 ..< 8).map { small.offsetBy(dx: CGFloat($0) * 32, dy: -CGFloat($0) * 32) }
        #expect(MirrorWindowSizing.cascaded(small, screens: [screen], peers: Array(crowded.prefix(7))) == small.offsetBy(dx: 224, dy: -224))
        #expect(MirrorWindowSizing.cascaded(small, screens: [screen], peers: crowded) == small)
    }

    /// Only peers on the window's screen count.
    @Test func peersOnAnotherScreenDoNotCount() {
        let second = MirrorWindowSizing.Screen(
            frame: CGRect(x: 1600, y: 0, width: 1000, height: 800),
            visible: CGRect(x: 1600, y: 0, width: 1000, height: 775)
        )
        let frame = CGRect(x: 1300, y: 200, width: 300, height: 600)
        // Mostly on the second screen, its top-left corner at the window's.
        let peer = CGRect(x: 1302, y: 200, width: 1000, height: 600)
        #expect(MirrorWindowSizing.cascaded(frame, screens: [screen, second], peers: [peer]) == frame)

        #expect(MirrorWindowSizing.screenIndex(for: peer, among: [screen.frame, second.frame]) == 1)
        #expect(MirrorWindowSizing.screenIndex(for: CGRect(x: 2700, y: 0, width: 10, height: 10), among: [screen.frame, second.frame]) == 1, "the nearest")
    }

    /// A kept frame is used as it was when it overlaps a screen, out of the visible area or not.
    @Test func aKeptFrameReturnsAsItWas() {
        let screens = [screen.frame]
        let inside = CGRect(x: 10, y: 10, width: 300, height: 652)
        #expect(MirrorWindowSizing.restoredFrame(inside, screens: screens) == inside)
        let across = CGRect(x: 1500, y: -20, width: 300, height: 1200)
        #expect(MirrorWindowSizing.restoredFrame(across, screens: screens) == across, "not moved in")
        #expect(MirrorWindowSizing.restoredFrame(CGRect(x: 1700, y: 0, width: 300, height: 652), screens: screens) == nil, "on no screen")
        #expect(MirrorWindowSizing.restoredFrame(CGRect(x: 1600, y: 0, width: 300, height: 652), screens: screens) == nil, "touching is not overlapping")
        #expect(MirrorWindowSizing.restoredFrame(.zero, screens: screens) == nil)
    }

    @Test func aVideoOfTheSameAspectChangesNothing() {
        let portrait = CGRect(x: 1000, y: 100, width: 360, height: 852)
        let frame = MirrorWindowSizing.reshaped(
            content: CGSize(width: 362, height: 800), current: portrait, fallbackAspect: 0.45, previous: nil, visible: visible, nonClient: nonClient
        )
        #expect(frame == nil, "0.4525 is within 0.01 of 0.45")
    }

    /// A video that fits between the smallest window and 75 % of the largest one takes its own size,
    /// the window keeping its top-right corner.
    @Test func aNewAspectTakesTheVideosOwnSize() {
        let portrait = CGRect(x: 1000, y: 100, width: 360, height: 852)
        let frame = MirrorWindowSizing.reshaped(
            content: CGSize(width: 400, height: 300), current: portrait, fallbackAspect: 0.45, previous: nil, visible: visible, nonClient: nonClient
        )
        #expect(frame == CGRect(x: 960, y: 600, width: 400, height: 352))
    }

    /// A video larger than 75 % of the largest window of its aspect shrinks to it; one smaller than
    /// the smallest window grows to it.
    @Test func aNewAspectStaysBetweenTheLimits() throws {
        let portrait = CGRect(x: 1000, y: 100, width: 360, height: 852)
        let large = try #require(MirrorWindowSizing.reshaped(
            content: CGSize(width: 2400, height: 1080), current: portrait, fallbackAspect: 0.45, previous: nil, visible: visible, nonClient: nonClient
        ))
        // The largest 2.22 window is 1600 × 772: 75 % is 1200 × 579, 527 of video high.
        #expect(large.size == CGSize(width: 1171, height: 579))
        #expect(large.maxX == portrait.maxX && large.maxY == portrait.maxY)

        let tiny = try #require(MirrorWindowSizing.reshaped(
            content: CGSize(width: 100, height: 200), current: CGRect(x: 100, y: 100, width: 400, height: 352),
            fallbackAspect: 400.0 / 300.0, previous: nil, visible: visible, nonClient: nonClient
        ))
        #expect(tiny.size == CGSize(width: 240, height: 532), "240 wide for a portrait video")
    }

    /// The frame before the last reshape returns for its aspect when it fits the visible area, wherever
    /// the window is now.
    @Test func thePreviousFrameReturnsForItsAspect() {
        let portrait = CGRect(x: 1000, y: 100, width: 360, height: 852)
        let previous = MirrorWindowSizing.Placement(aspect: 0.45, frame: portrait)
        let landscape = CGRect(x: 50, y: 50, width: 1171, height: 579)
        let back = MirrorWindowSizing.reshaped(
            content: CGSize(width: 720, height: 1600), current: landscape, fallbackAspect: 2.2, previous: previous, visible: visible, nonClient: nonClient
        )
        #expect(back == portrait)

        let outside = MirrorWindowSizing.Placement(aspect: 0.45, frame: portrait.offsetBy(dx: 500, dy: 0))
        let other = MirrorWindowSizing.reshaped(
            content: CGSize(width: 720, height: 1600), current: landscape, fallbackAspect: 2.2, previous: outside, visible: visible, nonClient: nonClient
        )
        #expect(other != outside.frame, "it no longer fits the visible area")
    }

    /// Top-right, top-left, bottom-left, bottom-right; else the top-left corner moved in.
    @Test func theWindowKeepsTheFirstCornerThatFits() {
        let size = CGSize(width: 400, height: 352)
        let topLeft = MirrorWindowSizing.anchored(size, current: CGRect(x: 0, y: 600, width: 300, height: 400), visible: visible)
        #expect(topLeft == CGPoint(x: 0, y: 648))

        let bottomLeft = MirrorWindowSizing.anchored(size, current: CGRect(x: 0, y: 0, width: 300, height: 200), visible: visible)
        #expect(bottomLeft == CGPoint(x: 0, y: 0))

        let off = MirrorWindowSizing.anchored(size, current: CGRect(x: -200, y: -200, width: 300, height: 300), visible: visible)
        #expect(off == CGPoint(x: 0, y: 0))
        let high = MirrorWindowSizing.anchored(size, current: CGRect(x: 1500, y: 900, width: 300, height: 300), visible: visible)
        #expect(high == CGPoint(x: 1200, y: 648), "its top edge moved down to the visible area's")
    }

    @Test func theSmallestWindowFollowsTheAspect() {
        #expect(MirrorWindowSizing.minimumSize(aspect: 0.45, visible: visible, nonClient: nonClient) == CGSize(width: 240, height: 585))
        #expect(MirrorWindowSizing.minimumSize(aspect: 1, visible: visible, nonClient: nonClient) == CGSize(width: 240, height: 292), "1 is portrait")
        #expect(MirrorWindowSizing.minimumSize(aspect: 2, visible: visible, nonClient: nonClient) == CGSize(width: 296, height: 200))
        #expect(MirrorWindowSizing.minimumSize(aspect: 1.2, visible: visible, nonClient: nonClient) == CGSize(width: 240, height: 200), "at least 240 wide")
        let short = CGRect(x: 0, y: 0, width: 1600, height: 400)
        #expect(MirrorWindowSizing.minimumSize(aspect: 0.3, visible: short, nonClient: nonClient).height == 400, "at most the visible height")
    }

    @Test func resizingKeepsTheAspectByTheDraggedEdge() {
        func resized(_ size: CGSize, _ edge: MirrorWindowSizing.ResizeEdge?, _ aspect: CGFloat) -> CGSize {
            MirrorWindowSizing.liveResized(size, edge: edge, aspect: aspect, visible: visible, nonClient: nonClient)
        }

        #expect(resized(CGSize(width: 400, height: 900), .horizontal, 0.45) == CGSize(width: 400, height: 941))
        #expect(resized(CGSize(width: 400, height: 600), .vertical, 0.45) == CGSize(width: 247, height: 600))
        #expect(resized(CGSize(width: 500, height: 600), .corner, 0.45) == CGSize(width: 247, height: 600), "a portrait corner follows the height")
        #expect(resized(CGSize(width: 800, height: 900), .corner, 2) == CGSize(width: 800, height: 452), "a landscape corner follows the width")

        // Capped at the visible area, the other side following.
        #expect(resized(CGSize(width: 500, height: 900), .horizontal, 0.45) == CGSize(width: 427, height: 1000))
        #expect(resized(CGSize(width: 1700, height: 500), .horizontal, 2) == CGSize(width: 1600, height: 852))
    }

    @Test func theDraggedEdgeIsTheSideThatMoved() {
        let start = CGSize(width: 400, height: 900)
        func edge(_ proposed: CGSize, current: CGSize = CGSize(width: 400, height: 900), _ known: MirrorWindowSizing.ResizeEdge? = nil) -> MirrorWindowSizing.ResizeEdge? {
            MirrorWindowSizing.resizeEdge(proposed: proposed, start: start, current: current, known: known)
        }

        #expect(edge(CGSize(width: 410, height: 900)) == .horizontal)
        #expect(edge(CGSize(width: 400, height: 950)) == .vertical)
        #expect(edge(CGSize(width: 410, height: 910)) == .corner)
        #expect(edge(start) == nil)

        // The followed side matching the window now, or the start, is not dragged.
        #expect(edge(CGSize(width: 420, height: 963), current: CGSize(width: 410, height: 963), .horizontal) == .horizontal)
        #expect(edge(CGSize(width: 420, height: 900), current: CGSize(width: 410, height: 963), .horizontal) == .horizontal)
        // A corner stays a corner.
        #expect(edge(CGSize(width: 420, height: 900), .corner) == .corner)
        #expect(edge(CGSize(width: 420, height: 920), current: CGSize(width: 410, height: 963), .horizontal) == .corner)
    }

    @Test func theNavigationBarChangesTheHeightKeepingTheTop() {
        let frame = CGRect(x: 100, y: 100, width: 360, height: 852)
        let barred = CGSize(width: 0, height: 84)
        let grown = MirrorWindowSizing.heightChanged(frame, by: 32, aspect: 0.45, visible: visible, nonClient: barred)
        #expect(grown == CGRect(x: 100, y: 68, width: 360, height: 884))
        let capped = MirrorWindowSizing.heightChanged(frame, by: 500, aspect: 0.45, visible: visible, nonClient: barred)
        #expect(capped.height == 1000 && capped.maxY == frame.maxY)

        // A window whose video area without the bar has the aspect already holds it.
        #expect(MirrorWindowSizing.holdsNavigationBar(grown, nonClientWithoutBar: nonClient, aspect: 0.45))
        #expect(!MirrorWindowSizing.holdsNavigationBar(frame, nonClientWithoutBar: nonClient, aspect: 0.45))
    }

    @Test func zoomFillsTheScreenWithTheAspect() {
        let frame = MirrorWindowSizing.zoomedFrame(aspect: 0.5, visible: visible, nonClient: nonClient)
        #expect(frame == CGRect(x: 563, y: 0, width: 474, height: 1000))
    }
}

struct MirrorWindowFramesTests {
    /// A store in a defaults domain of its own.
    private func store() -> (MirrorWindowFrames, UserDefaults, String) {
        let suite = "MirrorWindowFramesTests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        return (MirrorWindowFrames(defaults: defaults), defaults, suite)
    }

    @Test func layoutsMapToTwoSlots() {
        #expect(MirrorWindowFrames.slot(2) == .expanded)
        #expect(MirrorWindowFrames.slot(1) == .standard)
        #expect(MirrorWindowFrames.slot(0) == .standard)

        // Saved under the window's layout, else (unknown or Default) the app's last one.
        #expect(MirrorWindowFrames.saveLayout(current: 1, last: 2) == 1)
        #expect(MirrorWindowFrames.saveLayout(current: 0, last: 2) == 2)
        #expect(MirrorWindowFrames.saveLayout(current: nil, last: 2) == 2)
        #expect(MirrorWindowFrames.saveLayout(current: nil, last: nil) == 0)
    }

    /// Expanded keeps a frame of its own; Default and Compact share one. A new window takes the frame
    /// of the app's last layout, Expanded falling back to the Default one.
    @Test func eachSlotKeepsItsFrame() {
        let (frames, defaults, suite) = store()
        defer { defaults.removePersistentDomain(forName: suite) }

        let standard = CGRect(x: 10, y: 20, width: 300, height: 652)
        let expanded = CGRect(x: 40, y: 50, width: 900, height: 652)
        #expect(frames.openingFrame(package: "com.app", lastLayout: 2) == nil)

        frames.save(standard, package: "com.app", slot: .standard)
        #expect(frames.frame(package: "com.app", slot: .expanded) == nil, "a slot asked for by name has no fallback")
        #expect(frames.openingFrame(package: "com.app", lastLayout: 2) == standard)
        #expect(frames.openingFrame(package: "com.app", lastLayout: 1) == standard)

        frames.save(expanded, package: "com.app", slot: .expanded)
        #expect(frames.openingFrame(package: "com.app", lastLayout: 2) == expanded)
        #expect(frames.openingFrame(package: "com.app", lastLayout: 0) == standard)
        #expect(frames.openingFrame(package: "com.other", lastLayout: 0) == nil, "per app")

        defaults.set("garbage", forKey: "MirrorWindowFrame.com.app.expanded")
        #expect(frames.openingFrame(package: "com.app", lastLayout: 2) == standard, "unreadable falls back")
    }
}
