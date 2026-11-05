import Testing
@testable import PhoneLinkKit

@MainActor
struct MirrorVolumePopupTests {
    /// A popup at 50, unmuted, whose sends are recorded in `sent`.
    private func popup(_ sent: SentVolumes, volume: Int32 = 50, muted: Bool = false) -> MirrorVolumePopup {
        MirrorVolumePopup(state: MirrorVolumePopup.State(volume: volume, muted: muted)) { sent.states.append($0) }
    }

    @Test func aMoveIsSentOnlyWhenCommitted() {
        let sent = SentVolumes()
        let volume = popup(sent)
        #expect(volume.slider == 50)

        volume.moved(to: 70)
        #expect(sent.states.isEmpty)
        volume.commit()
        #expect(sent.states == [.init(volume: 70, muted: false)])

        volume.commit()
        #expect(sent.states.count == 1, "nothing is pending any more")
        volume.moved(to: 0)
        volume.commit()
        #expect(sent.states.last == .init(volume: 0, muted: true), "0 is muted")
    }

    /// A value back at the last committed (or received) one is no change.
    @Test func aMoveBackIsNoChange() {
        let sent = SentVolumes()
        let volume = popup(sent)
        volume.moved(to: 70)
        volume.moved(to: 50)
        volume.commit()
        #expect(sent.states.isEmpty)

        volume.moved(to: 60)
        volume.commit()
        volume.moved(to: 60.2)
        volume.commit()
        #expect(sent.states == [.init(volume: 60, muted: false)], "60.2 is 60, as committed")
    }

    /// A state from the phone drops the change not yet committed and moves the slider; the same state
    /// again does neither.
    @Test func aReceivedStateDropsThePendingChange() {
        let sent = SentVolumes()
        let volume = popup(sent)
        volume.moved(to: 80)
        volume.receive(.init(volume: 50, muted: false))
        volume.commit()
        #expect(sent.states == [.init(volume: 80, muted: false)], "the same state changes nothing")

        volume.moved(to: 90)
        volume.receive(.init(volume: 30, muted: false))
        #expect(volume.slider == 30 && volume.state == .init(volume: 30, muted: false))
        volume.commit()
        #expect(sent.states.count == 1)

        volume.moved(to: 10)
        volume.cancel()
        volume.commit()
        #expect(sent.states.count == 1, "cancelled")
    }

    /// The mute button turns the received state's mute over at once, whatever the slider shows, and
    /// drops the slider's change.
    @Test func muteUsesTheReceivedVolume() {
        let sent = SentVolumes()
        let volume = popup(sent)
        volume.moved(to: 60)
        volume.toggleMute()
        #expect(sent.states == [.init(volume: 50, muted: true)])
        #expect(volume.state.muted && volume.slider == 50)

        volume.commit()
        #expect(sent.states.count == 1, "the slider's change went with it")
        volume.toggleMute()
        #expect(sent.states.last == .init(volume: 50, muted: false))
    }

    /// A notch moves the slider by 2; the change goes half a second after the last notch, and
    /// scrolling counts even where the value ends where it was.
    @Test func scrollingCommitsAfterItSettles() async throws {
        let sent = SentVolumes()
        let volume = popup(sent)
        volume.scrolled(up: true)
        volume.scrolled(up: true)
        volume.scrolled(up: false)
        volume.scrolled(up: true)
        #expect(volume.slider == 54)
        #expect(sent.states.isEmpty)

        try await Task.sleep(for: MirrorVolumePopup.scrollSettle + .milliseconds(300))
        #expect(sent.states == [.init(volume: 54, muted: false)])

        volume.receive(.init(volume: 100, muted: false))
        volume.scrolled(up: true)
        #expect(volume.slider == 100, "at the top")
        volume.commit()
        #expect(sent.states.last == .init(volume: 100, muted: false))

        volume.receive(.init(volume: 1, muted: false))
        volume.scrolled(up: false)
        volume.commit()
        #expect(sent.states.last == .init(volume: 0, muted: true), "down to 0")
    }

    /// A commit before the scrolling settles sends it at once and only once.
    @Test func anEarlyCommitEndsTheScrolling() async throws {
        let sent = SentVolumes()
        let volume = popup(sent)
        volume.scrolled(up: false)
        volume.commit()
        #expect(sent.states == [.init(volume: 48, muted: false)])

        try await Task.sleep(for: MirrorVolumePopup.scrollSettle + .milliseconds(300))
        #expect(sent.states.count == 1)
    }
}

@MainActor
private final class SentVolumes {
    var states: [MirrorVolumePopup.State] = []
}
