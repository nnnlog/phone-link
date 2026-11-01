import PhoneLinkEngine
import PhoneLinkKit
import SwiftUI

/// A mirroring window's content: the phone's video (or app's) with its input, the takeover the
/// window's state calls for, a moment's notice when the app does not let its audio be captured, and
/// the toolbar — Back while the video shows, Home and Recents while the phone navigates by gestures,
/// the app's volume once it plays sound (or *Enable audio* while its audio is off on the phone), its
/// other layout, and the phone's keyboard settings.
struct MirrorWindowView: View {
    let sessionId: String
    let video: MirrorVideoLayer
    @Environment(ScreenMirroringModel.self) private var model

    var body: some View {
        let session = model.session(sessionId)
        ZStack {
            MirrorVideoRepresentable(
                sessionId: sessionId,
                video: video,
                model: model,
                display: session.map(MirrorDisplay.init) ?? MirrorDisplay(),
                inputEnabled: session.map(model.takesInput) ?? false
            )
            if let session, let overlay = model.overlay(for: session) {
                MirrorTakeover(session: session, overlay: overlay)
            }
        }
        .overlay(alignment: .top) {
            if model.showsCaptureNotice(sessionId) {
                MirrorCaptureNotice()
                    .padding(.top, 12)
                    .transition(.opacity)
            }
        }
        .animation(.default, value: model.showsCaptureNotice(sessionId))
        // The window sets the content's size and its minimum (it keeps the video's aspect).
        .toolbar {
            if let session {
                MirrorToolbar(session: session)
            }
        }
    }
}

/// The video view, kept in step with the session's display state.
private struct MirrorVideoRepresentable: NSViewRepresentable {
    let sessionId: String
    let video: MirrorVideoLayer
    let model: ScreenMirroringModel
    let display: MirrorDisplay
    let inputEnabled: Bool

    func makeNSView(context: Context) -> MirrorInputView {
        let view = MirrorInputView(sessionId: sessionId, video: video)
        view.model = model
        return view
    }

    func updateNSView(_ view: MirrorInputView, context: Context) {
        view.display = display
        let enabled = inputEnabled && !view.inputEnabled
        view.inputEnabled = inputEnabled
        if enabled {
            // The takeover's buttons may have held the focus.
            view.window?.makeFirstResponder(view)
        }
    }
}

/// The window's takeover: a spinner, or a message with its buttons, over a dimmed video.
private struct MirrorTakeover: View {
    let session: MirrorSessionView
    let overlay: MirrorOverlay
    @Environment(ScreenMirroringModel.self) private var model

    var body: some View {
        ZStack {
            Rectangle()
                .fill(.background.opacity(0.92))
            content
                .padding(24)
                .frame(maxWidth: 360)
        }
    }

    @ViewBuilder private var content: some View {
        switch overlay {
        case let .loading(text, slow):
            MirrorLoading(text: text, slow: slow)
        case let .progress(text):
            VStack(spacing: 12) {
                ProgressView()
                Text(text)
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
            }
        case let .message(symbol, title, message, buttons):
            VStack(spacing: 12) {
                Image(systemName: symbol)
                    .font(.system(size: 36))
                    .foregroundStyle(.secondary)
                Text(title)
                    .font(.title3.weight(.semibold))
                    .multilineTextAlignment(.center)
                Text(message)
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
                if !buttons.isEmpty {
                    HStack {
                        ForEach(Array(buttons.enumerated()), id: \.offset) { index, button in
                            if index == 0 {
                                Button(button.title) { model.perform(button.action, in: session.id) }
                                    .buttonStyle(.borderedProminent)
                                    .keyboardShortcut(.defaultAction)
                            } else {
                                Button(button.title) { model.perform(button.action, in: session.id) }
                            }
                        }
                    }
                    .padding(.top, 4)
                }
            }
        case .nearby:
            MirrorNearbyCheck()
        case let .moveToMac(taskTransfer):
            MirrorMoveToMac(session: session, taskTransfer: taskTransfer)
        case .layoutSwitch:
            MirrorLayoutSwitch(session: session)
        case .trueTouch:
            MirrorTrueTouch(session: session)
        }
    }
}

/// The window waits for the phone; after a while its line says the wait is long.
private struct MirrorLoading: View {
    let text: String
    let slow: String
    @State private var isSlow = false

    var body: some View {
        VStack(spacing: 12) {
            ProgressView()
            Text(isSlow ? slow : text)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
        }
        .task(id: text) {
            isSlow = false
            try? await Task.sleep(for: MirrorOverlay.slowAfter)
            if !Task.isCancelled { isSlow = true }
        }
    }
}

/// The phone asks to confirm that its user touches it.
private struct MirrorTrueTouch: View {
    let session: MirrorSessionView
    @Environment(ScreenMirroringModel.self) private var model
    @State private var doNotShow = false

    var body: some View {
        let name = model.title(for: session)
        VStack(spacing: 12) {
            Image(systemName: "hand.raised")
                .font(.system(size: 36))
                .foregroundStyle(.secondary)
            Text("Continue on This Mac?")
                .font(.title3.weight(.semibold))
            Text("\(name) wants to confirm that you're touching \(model.phoneDisplayName). It may behave differently when you use it from this Mac.")
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
            Toggle("Don't ask again", isOn: $doNotShow)
                .toggleStyle(.checkbox)
            HStack {
                Button("Cancel") { model.perform(.close, in: session.id) }
                Button("Continue") { model.continueTrueTouch(session.id, doNotShowAgain: doNotShow) }
                    .buttonStyle(.borderedProminent)
                    .keyboardShortcut(.defaultAction)
            }
        }
    }
}

/// The phone asks to move the app onto this Mac.
private struct MirrorMoveToMac: View {
    let session: MirrorSessionView
    let taskTransfer: Bool
    @Environment(ScreenMirroringModel.self) private var model
    @State private var doNotAsk = false

    var body: some View {
        let name = model.title(for: session)
        VStack(spacing: 12) {
            Image(systemName: "macbook.and.iphone")
                .font(.system(size: 36))
                .foregroundStyle(.secondary)
            Text(taskTransfer ? "Continue on This Mac?" : "Open \(name) on This Mac?")
                .font(.title3.weight(.semibold))
                .multilineTextAlignment(.center)
            Text(taskTransfer
                ? "\(name) opens in this window where you left off on \(model.phoneDisplayName)."
                : "\(model.phoneDisplayName) restarts \(name) in this window.")
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
            Toggle("Don't ask again", isOn: $doNotAsk)
                .toggleStyle(.checkbox)
            HStack {
                Button("Not Now") { model.answerMoveToMac(session.id, accept: false, doNotAsk: doNotAsk) }
                Button(taskTransfer ? "Continue" : "Open") {
                    model.answerMoveToMac(session.id, accept: true, doNotAsk: doNotAsk)
                }
                .buttonStyle(.borderedProminent)
                .keyboardShortcut(.defaultAction)
            }
        }
    }
}

/// The user's layout switch restarts the app; it waits for the user to confirm.
private struct MirrorLayoutSwitch: View {
    let session: MirrorSessionView
    @Environment(ScreenMirroringModel.self) private var model
    @State private var doNotAsk = false

    var body: some View {
        VStack(spacing: 12) {
            Image(systemName: "rectangle.portrait.arrowtriangle.2.outward")
                .font(.system(size: 36))
                .foregroundStyle(.secondary)
            Text("Change the Layout?")
                .font(.title3.weight(.semibold))
            Text("\(model.title(for: session)) restarts on \(model.phoneDisplayName) in the new layout.")
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
            Toggle("Don't ask again", isOn: $doNotAsk)
                .toggleStyle(.checkbox)
            Button("Change Layout") { model.confirmLayoutSwitch(session.id, doNotAsk: doNotAsk) }
                .buttonStyle(.borderedProminent)
                .keyboardShortcut(.defaultAction)
        }
    }
}

/// The nearby check: the phone must be near this Mac or verify it with a code.
private struct MirrorNearbyCheck: View {
    @Environment(ScreenMirroringModel.self) private var model
    @State private var pin = ""
    @State private var enteringPin = false

    var body: some View {
        let prompt = model.status?.nearby
        VStack(spacing: 12) {
            Image(systemName: "lock.iphone")
                .font(.system(size: 36))
                .foregroundStyle(.secondary)
            if let prompt {
                if prompt.skipUnlock {
                    Text("Unlock Your Phone")
                        .font(.title3.weight(.semibold))
                    Text("Unlock \(model.phoneDisplayName), or continue without unlocking it.")
                        .foregroundStyle(.secondary)
                        .multilineTextAlignment(.center)
                    Button("Continue") { model.nearbySkipUnlock() }
                        .buttonStyle(.borderedProminent)
                } else if prompt.skipVerification {
                    Text("Your Phone Is Verified")
                        .font(.title3.weight(.semibold))
                    Button("Continue") { model.nearbySkipVerification() }
                        .buttonStyle(.borderedProminent)
                } else {
                    verification(prompt)
                }
            } else {
                ProgressView()
                Text("Checking that \(model.phoneDisplayName) is nearby…")
                    .foregroundStyle(.secondary)
            }
        }
    }

    @ViewBuilder
    private func verification(_ prompt: MirrorNearbyPrompt) -> some View {
        Text("Verify It's You")
            .font(.title3.weight(.semibold))
        if enteringPin {
            Text(prompt.pinCodeWrong
                ? "That code isn't right. Enter the code \(model.phoneDisplayName) shows."
                : "Enter the code \(model.phoneDisplayName) shows.")
                .foregroundStyle(prompt.pinCodeWrong ? AnyShapeStyle(.red) : AnyShapeStyle(.secondary))
                .multilineTextAlignment(.center)
            HStack {
                TextField("Code", text: $pin)
                    .textFieldStyle(.roundedBorder)
                    .frame(width: 120)
                    .onSubmit(send)
                Button("Verify", action: send)
                    .buttonStyle(.borderedProminent)
                    .disabled(pin.trimmingCharacters(in: .whitespaces).isEmpty)
            }
        } else if prompt.tokenExpired {
            Text("The code expired.")
                .foregroundStyle(.secondary)
            Button("Show a New Code") { model.nearbyCodeRefreshed() }
                .buttonStyle(.borderedProminent)
        } else {
            // The phone's QR verification carries this token in a payload whose format is not known, so
            // the Mac offers only the code from the phone.
            Text("\(model.phoneDisplayName) isn't on the same network as this Mac. Verify with a code from your phone.")
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
        }
        if !enteringPin {
            Button("Use a Code from Your Phone Instead") {
                enteringPin = true
                model.nearbyRequestPinCode()
            }
            .buttonStyle(.link)
        }
    }

    private func send() {
        model.nearbySendPinCode(pin)
        pin = ""
    }
}

/// The app does not let its audio be captured, so none plays on this Mac.
private struct MirrorCaptureNotice: View {
    var body: some View {
        Label("This app doesn't let its audio play on other devices", systemImage: "speaker.slash")
            .font(.callout)
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
            .background(.regularMaterial, in: Capsule())
            .shadow(radius: 4)
            .allowsHitTesting(false)
    }
}

/// The window's toolbar.
private struct MirrorToolbar: ToolbarContent {
    let session: MirrorSessionView
    @Environment(ScreenMirroringModel.self) private var model

    var body: some ToolbarContent {
        ToolbarItemGroup(placement: .navigation) {
            if model.showsBack(session) {
                Button {
                    model.back(session.id)
                } label: {
                    Label("Back", systemImage: "chevron.backward")
                }
                .help("Back")
            }
            if model.showsNavigation(session) {
                Button {
                    model.navButton(session.id, .home)
                } label: {
                    Label("Home", systemImage: "circle")
                }
                .help("Home")
                Button {
                    model.navButton(session.id, .recents)
                } label: {
                    Label("Recent Apps", systemImage: "square.on.square")
                }
                .help("Recent Apps")
            }
        }
        ToolbarItemGroup(placement: .primaryAction) {
            switch model.audioButton(session) {
            case .volume:
                MirrorVolumeControl(session: session)
            case .enableAudio:
                Button {
                    model.enableAudio(session.id)
                } label: {
                    Label("Turn On Audio", systemImage: "speaker.slash")
                }
                .help("Turn on this app's audio on your phone")
            case .none:
                EmptyView()
            }
            if model.offersLayouts(session) {
                let compact = session.screenLayout == 1
                Button {
                    model.switchLayout(session.id, to: model.otherLayout(session))
                } label: {
                    Label(
                        compact ? "Switch to Tablet Layout" : "Switch to Phone Layout",
                        systemImage: compact ? "ipad.landscape" : "iphone"
                    )
                }
                .help(compact ? "Switch to Tablet Layout" : "Switch to Phone Layout")
            }
            if model.status?.inputSupported == true {
                Button {
                    model.requestKeyboardSettings()
                } label: {
                    Label("Keyboard Settings", systemImage: "keyboard")
                }
                .help("Choose the keyboard language on your phone")
            }
        }
    }
}

/// The app's volume on the phone: a mute button and a slider in a popover.
private struct MirrorVolumeControl: View {
    let session: MirrorSessionView
    @Environment(ScreenMirroringModel.self) private var model
    @State private var shown = false
    @State private var volume: Double = 50

    var body: some View {
        Button {
            volume = Double(session.volume)
            shown.toggle()
        } label: {
            Label("Volume", systemImage: symbol)
        }
        .help("Volume")
        .popover(isPresented: $shown, arrowEdge: .bottom) {
            HStack(spacing: 8) {
                Button {
                    model.setVolume(session, volume: session.volume, muted: !session.muted)
                } label: {
                    Image(systemName: session.muted ? "speaker.slash.fill" : "speaker.fill")
                }
                .buttonStyle(.borderless)
                .help(session.muted ? "Unmute" : "Mute")
                Slider(value: $volume, in: 0 ... 100) { editing in
                    if !editing {
                        // A slider at 0 mutes; moving it up unmutes.
                        let level = Int32(volume.rounded())
                        model.setVolume(session, volume: level, muted: level == 0)
                    }
                }
                .frame(width: 160)
            }
            .padding(12)
        }
    }

    private var symbol: String {
        if session.muted || session.volume == 0 { return "speaker.slash" }
        return session.silent ? "speaker" : "speaker.wave.2"
    }
}
