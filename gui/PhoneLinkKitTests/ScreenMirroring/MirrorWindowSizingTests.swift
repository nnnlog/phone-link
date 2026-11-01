import CoreGraphics
import Testing
@testable import PhoneLinkKit

struct MirrorWindowSizingTests {
    let visible = CGRect(x: 0, y: 0, width: 1600, height: 1000)
    let chrome = CGSize(width: 0, height: 52)

    @Test func aNewWindowFillsThreeQuartersOfTheScreenCentred() {
        let frame = MirrorWindowSizing.initialFrame(aspect: 0.45, visible: visible, chrome: chrome)
        // 75 % of 1000 high is the tighter bound for a portrait phone.
        #expect(frame.height == CGFloat(802), "750 of content and 52 of chrome")
        #expect(frame.width == (750 * 0.45).rounded())
        #expect(abs(frame.midX - visible.midX) < 1 && abs(frame.midY - visible.midY) < 1)

        let wide = MirrorWindowSizing.initialFrame(aspect: 16.0 / 9.0, visible: visible, chrome: chrome)
        #expect(wide.width - chrome.width <= 1200 && wide.height - chrome.height <= 750)
    }

    @Test func turningKeepsTheHeightWhenWiderAndTheTopRightCorner() {
        let portrait = CGRect(x: 1000, y: 100, width: 360, height: 852)
        let landscape = MirrorWindowSizing.resizedFrame(aspect: 2.2, current: portrait, earlier: [], visible: visible, chrome: chrome)
        // Wider: the content keeps its 800 points of height, but 1760 wide is more than 75 % of the
        // screen, so it shrinks to 1200 wide.
        #expect(landscape.width == 1200)
        #expect(landscape.height == CGFloat((1200 / 2.2).rounded() + 52))
        // The top-right corner stays where it was.
        #expect(visible.contains(landscape))
        #expect(landscape.maxX == portrait.maxX && landscape.maxY == portrait.maxY)

        // Where that corner leaves no room, the next corner that does is kept, else the window moves in.
        let low = CGRect(x: 1000, y: 0, width: 360, height: 852)
        let moved = MirrorWindowSizing.resizedFrame(aspect: 1, current: low.offsetBy(dx: 0, dy: -100), earlier: [], visible: visible, chrome: chrome)
        #expect(visible.contains(moved))
    }

    @Test func narrowerKeepsTheWidthAndTheTopRightCorner() {
        let wide = CGRect(x: 200, y: 400, width: 800, height: 452)
        let square = MirrorWindowSizing.resizedFrame(aspect: 1, current: wide, earlier: [], visible: visible, chrome: chrome)
        #expect(square.width == CGFloat(750) && square.height == CGFloat(802), "800 wide shrinks to 75 % of the height")
        #expect(square.maxX == wide.maxX && square.maxY == wide.maxY)
    }

    @Test func turningBackRestoresTheEarlierFrame() {
        let portrait = CGRect(x: 1000, y: 100, width: 360, height: 852)
        let landscape = MirrorWindowSizing.resizedFrame(aspect: 2.2, current: portrait, earlier: [], visible: visible, chrome: chrome)
        let back = MirrorWindowSizing.resizedFrame(aspect: 0.45, current: landscape, earlier: [portrait], visible: visible, chrome: chrome)
        #expect(back == portrait)

        // Moved away from the earlier frame's corners, it is not taken again.
        let moved = landscape.offsetBy(dx: -50, dy: -50)
        let other = MirrorWindowSizing.resizedFrame(aspect: 0.45, current: moved, earlier: [portrait], visible: visible, chrome: chrome)
        #expect(other != portrait)
    }

    @Test func aSmallWindowGrowsToTheMinimum() {
        let tiny = CGRect(x: 100, y: 100, width: 100, height: 152)
        let frame = MirrorWindowSizing.resizedFrame(aspect: 0.45, current: tiny, earlier: [], visible: visible, chrome: chrome)
        #expect(frame.width == 280, "a narrow window is at least 280 points wide")

        let minimum = MirrorWindowSizing.minimumContent(aspect: 0.45, visible: visible, chrome: chrome)
        #expect(minimum.width == 280)
        let wideMinimum = MirrorWindowSizing.minimumContent(aspect: 2, visible: visible, chrome: chrome)
        #expect(wideMinimum.height == CGFloat(188), "a wide window is at least 240 points high (240 − 52 of chrome)")
    }

    /// Narrow and wide are measured against the visible area's aspect: on a 1.6 screen a 1.5 video is
    /// narrow, so its window is at least 280 points wide.
    @Test func theMinimumFollowsTheScreensAspect() {
        let minimum = MirrorWindowSizing.minimumContent(aspect: 1.5, visible: visible, chrome: chrome)
        #expect(minimum.width == 280 && abs(minimum.height - 280 / 1.5) < 0.001)

        let tall = CGRect(x: 0, y: 0, width: 1000, height: 1600)
        let wideOnTall = MirrorWindowSizing.minimumContent(aspect: 0.7, visible: tall, chrome: chrome)
        #expect(wideOnTall.height == CGFloat(188), "0.7 is wider than a 0.625 screen")
    }

    @Test func zoomFillsTheScreenWithTheAspect() {
        let frame = MirrorWindowSizing.zoomedFrame(aspect: 0.5, visible: visible, chrome: chrome)
        #expect(frame.height == 1000 && frame.width == 474)
        #expect(visible.contains(frame))
    }

    @Test func aSavedFrameReturnsMovedOntoItsScreen() {
        let screen = MirrorWindowSizing.Screen(frame: CGRect(x: 0, y: 0, width: 1600, height: 1040), visible: visible)
        func restored(_ frame: CGRect, deviceAspect: CGFloat? = nil, rightToLeft: Bool = false) -> CGRect? {
            MirrorWindowSizing.restoredFrame(frame, screens: [screen], chrome: chrome, deviceAspect: deviceAspect, rightToLeft: rightToLeft)
        }

        let inside = CGRect(x: 10, y: 10, width: 300, height: 652)
        #expect(restored(inside) == inside)
        // Sticking out to the right and below: moved in, size kept.
        #expect(restored(CGRect(x: 1500, y: -20, width: 300, height: 652)) == CGRect(x: 1300, y: 0, width: 300, height: 652))
        // Into the menu bar: moved down.
        #expect(restored(CGRect(x: 10, y: 500, width: 300, height: 652)) == CGRect(x: 10, y: 348, width: 300, height: 652))
        // Wider than the visible area: left edge kept, cut to it; right-to-left moves the right edge first.
        #expect(restored(CGRect(x: -100, y: 0, width: 1800, height: 652)) == CGRect(x: 0, y: 0, width: 1600, height: 652))
        #expect(restored(CGRect(x: 1700, y: 0, width: 300, height: 652), rightToLeft: true) == nil, "on no screen")
        #expect(restored(CGRect(x: 1500, y: 0, width: 300, height: 652), rightToLeft: true) == CGRect(x: 1300, y: 0, width: 300, height: 652))
        #expect(restored(.zero) == nil)

        // With a device resolution known, the content's aspect must match it.
        #expect(restored(inside, deviceAspect: 300.0 / 600.0) == inside)
        #expect(restored(inside, deviceAspect: 1080.0 / 2400.0) == nil)

        // The screen it overlaps most is the one it moves into.
        let second = MirrorWindowSizing.Screen(
            frame: CGRect(x: 1600, y: 0, width: 1000, height: 800),
            visible: CGRect(x: 1600, y: 0, width: 1000, height: 775)
        )
        let across = CGRect(x: 1550, y: 100, width: 300, height: 652)
        let onSecond = MirrorWindowSizing.restoredFrame(across, screens: [screen, second], chrome: chrome, deviceAspect: nil)
        #expect(onSecond == CGRect(x: 1600, y: 100, width: 300, height: 652), "mostly on the second screen")
        let low = CGRect(x: 1550, y: 300, width: 300, height: 652)
        #expect(MirrorWindowSizing.restoredFrame(low, screens: [screen, second], chrome: chrome, deviceAspect: nil) == CGRect(x: 1600, y: 123, width: 300, height: 652))
    }
}
