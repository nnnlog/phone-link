import AppKit
import Combine
import PhoneLinkEngine
import PhoneLinkKit
import SwiftUI

/// The main Apple Glass UI window matching the reference design from airsync-mac.
struct MainWindow: View {
    @Environment(EngineController.self) private var controller
    @Environment(AppNavigator.self) private var navigator
    @Environment(DeviceStatusModel.self) private var deviceStatus
    @Environment(NotificationsModel.self) private var notifications
    @Environment(MessagesModel.self) private var messages
    @Environment(CallsModel.self) private var calls
    @Environment(ContactsModel.self) private var contacts
    @Environment(PhoneAppsModel.self) private var phoneApps
    @Environment(ScreenMirroringModel.self) private var mirroring
    @Environment(MediaControlsModel.self) private var media
    @Environment(WallpaperModel.self) private var wallpaper

    @Environment(\.colorScheme) private var colorScheme
    @State private var isSidebarVisible = true

    var body: some View {
        @Bindable var navigator = navigator
        Group {
            if controller.phase == .running && !controller.phoneUnlinked {
                connectedUI
            } else {
                ConnectionGateView()
            }
        }
        .frame(minWidth: 920, minHeight: 620)
        .navigationTitle("")
        .hideWindowToolbar()
        .onAppear {
            media.reload()
        }
    }

    /// The full glass UI, shown only while a phone is connected.
    private var connectedUI: some View {
        ZStack {
            // Apple Glass deep frosted background - true translucent glass
            VisualEffectView(material: .underWindowBackground, blendingMode: .behindWindow, state: .active)
                .ignoresSafeArea()

            // Subtle dynamic glass tint (matches AirSync windowOpacity)
            LinearGradient(
                colors: colorScheme == .dark ? [
                    Color.black.opacity(0.30),
                    Color.black.opacity(0.18)
                ] : [
                    Color.white.opacity(0.14),
                    Color.white.opacity(0.06)
                ],
                startPoint: .topLeading,
                endPoint: .bottomTrailing
            )
            .ignoresSafeArea()

            // Main layout
            HStack(spacing: 0) {
                if isSidebarVisible {
                    SidebarPhonePane(isSidebarVisible: $isSidebarVisible)
                        .frame(width: 310)
                        .transition(.move(edge: .leading).combined(with: .opacity))

                    // Translucent divider
                    Rectangle()
                        .fill(colorScheme == .dark ? Color.white.opacity(0.08) : Color.black.opacity(0.08))
                        .frame(width: 1)
                        .ignoresSafeArea()
                }

                // Right Main Content Pane
                MainContentPane(
                    isSidebarVisible: $isSidebarVisible,
                    section: Binding(
                        get: { navigator.section },
                        set: { navigator.section = $0 }
                    )
                )
            }
        }
    }
}

/// What the main window shows while the phone is not connected: sign-in when the Microsoft account
/// is not signed in, otherwise the QR pairing flow.
private struct ConnectionGateView: View {
    @Environment(EngineController.self) private var controller
    @Environment(AppNavigator.self) private var navigator

    var body: some View {
        ZStack {
            // A solid glass background, so the gate never shows as floating text over the desktop.
            LinearGradient(
                colors: [
                    Color(red: 0.13, green: 0.12, blue: 0.11),
                    Color(red: 0.07, green: 0.06, blue: 0.055)
                ],
                startPoint: .top,
                endPoint: .bottom
            )
            .ignoresSafeArea()

            VStack(spacing: 16) {
                switch controller.phase {
                case .idle, .starting:
                    ProgressView("Connecting to your phone…")
                case .webSignIn:
                    ContentUnavailableView {
                        Label("Sign In to Continue", systemImage: "person.crop.circle")
                    } description: {
                        Text("Finish signing in to your Microsoft account in the sign-in window.")
                    } actions: {
                        Button("Show Sign-In") { navigator.showSignIn() }
                            .buttonStyle(.borderedProminent)
                    }
                case .signedOut:
                    ContentUnavailableView {
                        Label("Signed Out", systemImage: "person.crop.circle")
                    } description: {
                        Text("Sign in with the Microsoft account your phone uses with Link to Windows.")
                    } actions: {
                        Button("Sign In") { controller.start() }
                            .buttonStyle(.borderedProminent)
                    }
                case let .failed(message):
                    ContentUnavailableView {
                        Label("Phone Link Couldn't Connect", systemImage: "exclamationmark.triangle")
                    } description: {
                        Text(message)
                    } actions: {
                        Button("Try Again") { controller.start() }
                            .buttonStyle(.borderedProminent)
                    }
                case .accountRemoved:
                    ContentUnavailableView {
                        Label("Sign In Again", systemImage: "person.crop.circle.badge.exclamationmark")
                    } description: {
                        Text("This Mac was removed from your Microsoft account's devices.")
                    } actions: {
                        Button("Sign In") { controller.start() }
                            .buttonStyle(.borderedProminent)
                    }
                case .selectingPhone, .running:
                    // No phone or not connected: pair a phone by QR.
                    VStack(spacing: 12) {
                        PairingView()
                        Text("Link your phone to continue.")
                            .font(.callout)
                            .foregroundStyle(.secondary)
                    }
                    .frame(maxWidth: 480)
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .padding(40)
        }
    }
}

// MARK: - Header Metrics
private enum LayoutMetrics {
    static let headerHeight: CGFloat = 52
}

// MARK: - Left Sidebar & Phone Mockup Pane

private struct SidebarPhonePane: View {
    @Binding var isSidebarVisible: Bool
    @Environment(\.colorScheme) private var colorScheme
    @Environment(EngineController.self) private var controller
    @Environment(DeviceStatusModel.self) private var deviceStatus
    @Environment(MediaControlsModel.self) private var media
    @Environment(ScreenMirroringModel.self) private var mirroring
    @Environment(WallpaperModel.self) private var wallpaper
    @Environment(AppNavigator.self) private var navigator

    var body: some View {
        ZStack {
            // Window Traffic Lights Area & Sidebar Toggle Button pinned at top
            VStack(spacing: 0) {
                HStack(spacing: 0) {
                    // Gap for standard macOS window buttons (close, minimize, zoom)
                    Spacer()
                        .frame(width: 72)

                    Spacer()

                    // Sidebar toggle icon button at trailing edge of sidebar
                    Button {
                        withAnimation(.spring(response: 0.35, dampingFraction: 0.8)) {
                            isSidebarVisible.toggle()
                        }
                    } label: {
                        Image(systemName: "sidebar.left")
                            .font(.system(size: 13, weight: .medium))
                            .foregroundStyle(Color.primary.opacity(0.85))
                            .frame(width: 28, height: 28)
                            .background(
                                RoundedRectangle(cornerRadius: 7, style: .continuous)
                                    .fill(colorScheme == .dark ? Color.white.opacity(0.08) : Color.black.opacity(0.06))
                            )
                    }
                    .buttonStyle(.plain)
                    .help("Toggle Sidebar")
                }
                .frame(height: LayoutMetrics.headerHeight)
                .padding(.horizontal, 14)

                Spacer()
            }

            // Centered Phone Assembly: perfectly vertically centered in the entire sidebar
            VStack(spacing: 10) {
                // Battery or Disconnected Status Pill (No dummy data)
                HStack(spacing: 8) {
                    if controller.connected {
                        if let percent = deviceStatus.summary?.batteryPercent {
                            HStack(spacing: 4) {
                                Image(systemName: batterySymbol)
                                    .font(.system(size: 11))
                                Text("\(percent)%")
                                    .font(.system(size: 11, weight: .medium, design: .rounded))
                            }
                        } else {
                            HStack(spacing: 5) {
                                Circle()
                                    .fill(Color.green)
                                    .frame(width: 6, height: 6)
                                Text("Connected")
                                    .font(.system(size: 11, weight: .medium, design: .rounded))
                            }
                        }
                        if let wifi = deviceStatus.summary?.wifi {
                            Image(systemName: wifi.symbol)
                                .font(.system(size: 10))
                        }
                    } else if isConnecting {
                        HStack(spacing: 6) {
                            ProgressView()
                                .controlSize(.mini)
                                .scaleEffect(0.7)
                            Text("Connecting…")
                                .font(.system(size: 11, weight: .medium, design: .rounded))
                                .foregroundStyle(Color.secondary)
                        }
                    } else {
                        HStack(spacing: 6) {
                            Circle()
                                .fill(Color.orange)
                                .frame(width: 6, height: 6)
                            Text("Not Connected")
                                .font(.system(size: 11, weight: .medium, design: .rounded))
                                .foregroundStyle(Color.secondary)
                        }
                    }
                }
                .foregroundStyle(Color.primary.opacity(0.9))
                .padding(.horizontal, 14)
                .padding(.vertical, 5)
                .background {
                    ZStack {
                        Capsule(style: .continuous)
                            .fill(.ultraThinMaterial)
                        Capsule(style: .continuous)
                            .fill(colorScheme == .dark ? Color.black.opacity(0.40) : Color.white.opacity(0.70))
                        Capsule(style: .continuous)
                            .strokeBorder(colorScheme == .dark ? Color.white.opacity(0.14) : Color.black.opacity(0.08), lineWidth: 0.8)
                    }
                }
                .clipShape(Capsule(style: .continuous))
                .shadow(color: Color.black.opacity(colorScheme == .dark ? 0.30 : 0.04), radius: 5, x: 0, y: 1.5)

                // Phone Mockup (Centerpiece with clock, album art/music card, and mirror button)
                PhoneMockupView()

                // Actions below mockup: Switch Device & Connect/Disconnect
                HStack(spacing: 8) {
                    // Switch / Pair device menu
                    Menu {
                        if !controller.phones.isEmpty {
                            Section("Linked Phones") {
                                ForEach(controller.phones, id: \.id) { phone in
                                    Button {
                                        if phone.id != controller.phone?.id {
                                            controller.selectPhone(phone.id)
                                        }
                                    } label: {
                                        HStack {
                                            Text(phone.name)
                                            if phone.id == controller.phone?.id {
                                                Image(systemName: "checkmark")
                                            }
                                        }
                                    }
                                }
                            }
                            Divider()
                        }
                        Button {
                            navigator.presentPairing()
                        } label: {
                            Label("Pair a New Phone…", systemImage: "qrcode")
                        }
                    } label: {
                        HStack(spacing: 4) {
                            Image(systemName: "arrow.triangle.2.circlepath")
                                .font(.system(size: 9.5, weight: .semibold))
                            Text("Switch")
                                .font(.system(size: 11, weight: .semibold))
                            Image(systemName: "chevron.down")
                                .font(.system(size: 7.5, weight: .bold))
                                .foregroundStyle(Color.secondary)
                        }
                        .foregroundStyle(Color.primary)
                        .padding(.horizontal, 12)
                        .padding(.vertical, 7)
                        .background {
                            ZStack {
                                Capsule(style: .continuous)
                                    .fill(.ultraThinMaterial)
                                Capsule(style: .continuous)
                                    .fill(colorScheme == .dark ? Color.white.opacity(0.10) : Color.white.opacity(0.75))
                                Capsule(style: .continuous)
                                    .strokeBorder(colorScheme == .dark ? Color.white.opacity(0.15) : Color.black.opacity(0.08), lineWidth: 0.8)
                            }
                        }
                        .clipShape(Capsule(style: .continuous))
                        .shadow(color: Color.black.opacity(colorScheme == .dark ? 0.25 : 0.04), radius: 4, x: 0, y: 1.5)
                    }
                    .menuStyle(.button)
                    .buttonStyle(.plain)
                    .menuIndicator(.hidden)
                    .help("Switch device or pair a new phone")

                    // Disconnect / Connect Pill Button
                    Button {
                        if controller.connected {
                            Task {
                                await controller.stop()
                            }
                        } else {
                            controller.refresh()
                        }
                    } label: {
                        HStack(spacing: 5) {
                            Image(systemName: controller.connected ? "xmark" : "arrow.clockwise")
                                .font(.system(size: 9.5, weight: .bold))
                            Text(controller.connected ? "Disconnect" : (isConnecting ? "Connecting…" : "Connect"))
                                .font(.system(size: 11, weight: .semibold))
                        }
                        .foregroundStyle(controller.connected ? Color.primary : Color.accentColor)
                        .padding(.horizontal, 12)
                        .padding(.vertical, 7)
                        .background {
                            ZStack {
                                Capsule(style: .continuous)
                                    .fill(.ultraThinMaterial)
                                Capsule(style: .continuous)
                                    .fill(
                                        controller.connected
                                            ? (colorScheme == .dark ? Color.white.opacity(0.12) : Color.white.opacity(0.80))
                                            : (colorScheme == .dark ? Color.accentColor.opacity(0.20) : Color.accentColor.opacity(0.12))
                                    )
                                Capsule(style: .continuous)
                                    .strokeBorder(
                                        controller.connected
                                            ? (colorScheme == .dark ? Color.white.opacity(0.18) : Color.black.opacity(0.08))
                                            : Color.accentColor.opacity(0.35),
                                        lineWidth: 0.8
                                    )
                            }
                        }
                        .clipShape(Capsule(style: .continuous))
                        .shadow(color: Color.black.opacity(colorScheme == .dark ? 0.30 : 0.04), radius: 4, x: 0, y: 1.5)
                    }
                    .buttonStyle(.plain)
                    .disabled(isConnecting)
                    .help(controller.connected ? "Disconnect Phone" : "Connect Phone")
                }
            }
            .padding(.top, 16)
        }
        .frame(maxHeight: .infinity)
    }

    private var batteryPercent: Int {
        Int(deviceStatus.summary?.batteryPercent ?? 0)
    }

    private var batterySymbol: String {
        guard let percent = deviceStatus.summary?.batteryPercent else { return "battery.100" }
        switch percent {
        case 0...10: return "battery.0"
        case 11...30: return "battery.25"
        case 31...60: return "battery.50"
        case 61...85: return "battery.75"
        default: return "battery.100"
        }
    }

    /// The engine is starting or running but hasn't established a phone session yet.
    private var isConnecting: Bool {
        switch controller.phase {
        case .starting, .running:
            return !controller.connected && controller.connectFailure == nil
        default:
            return false
        }
    }
}

// MARK: - Phone Mockup Frame & Screen

private struct PhoneMockupView: View {
    @Environment(WallpaperModel.self) private var wallpaper
    @Environment(ScreenMirroringModel.self) private var mirroring
    @Environment(MediaControlsModel.self) private var media
    @Environment(NotificationsModel.self) private var notifications
    @Environment(AppNavigator.self) private var navigator
    @Environment(EngineController.self) private var controller
    @Environment(\.colorScheme) private var colorScheme

    private let phoneWidth: CGFloat = 216
    private let phoneHeight: CGFloat = 436

    private var fallbackMediaSession: MediaControlsModel.Session? {
        guard controller.connected else { return nil }
        for n in notifications.notifications {
            let pkg = n.packageName.lowercased()
            let title = n.title ?? ""
            let isMedia = pkg.contains("music") || pkg.contains("spotify") || pkg.contains("youtube")
                || pkg.contains("audio") || pkg.contains("podcast") || pkg.contains("player")
                || pkg.contains("soundcloud") || pkg.contains("wynk") || pkg.contains("gaana")
            let hasMediaAction = n.actions.contains { action in
                let a = action.name.lowercased()
                return a.contains("play") || a.contains("pause") || a.contains("next") || a.contains("prev")
            }
            if (isMedia || hasMediaAction) && !title.isEmpty {
                let isPlaying = n.actions.contains { $0.name.lowercased().contains("pause") }
                return MediaControlsModel.Session(
                    id: Int64(-1),
                    appName: NotificationsModel.appName(of: n),
                    title: title,
                    artist: n.body.isEmpty ? nil : n.body,
                    albumArt: nil as NSImage?,
                    appIcon: nil as NSImage?,
                    playing: isPlaying
                )
            }
        }
        return nil
    }

    private var activeSession: MediaControlsModel.Session? {
        guard controller.connected else { return nil }
        if let s = media.selected, !s.title.isEmpty { return s }
        if let first = media.sessions.first, !first.title.isEmpty { return first }
        return fallbackMediaSession
    }

    var body: some View {
        ZStack {
            // Outer phone chassis
            RoundedRectangle(cornerRadius: 28, style: .continuous)
                .fill(Color(red: 0.08, green: 0.08, blue: 0.09))
                .overlay {
                    RoundedRectangle(cornerRadius: 28, style: .continuous)
                        .strokeBorder(
                            LinearGradient(
                                colors: [
                                    Color.white.opacity(0.35),
                                    Color.white.opacity(0.06),
                                    Color.white.opacity(0.18)
                                ],
                                startPoint: .topLeading,
                                endPoint: .bottomTrailing
                            ),
                            lineWidth: 1.5
                        )
                }
                .shadow(color: .black.opacity(0.50), radius: 18, x: 0, y: 10)

            // Inner Screen Area
            ZStack {
                // Wallpaper layer: if music is playing with album art, show it (as in AirSync PhoneView)
                if let session = activeSession, session.playing, let art = session.albumArt {
                    Image(nsImage: art)
                        .resizable()
                        .scaledToFill()
                        .frame(width: phoneWidth - 10, height: phoneHeight - 10)
                        .overlay(
                            LinearGradient(
                                colors: [Color.black.opacity(0.45), Color.black.opacity(0.15)],
                                startPoint: .top,
                                endPoint: .bottom
                            )
                        )
                        .clipped()
                } else if let image = wallpaper.image {
                    Image(nsImage: image)
                        .resizable()
                        .scaledToFill()
                        .frame(width: phoneWidth - 10, height: phoneHeight - 10)
                        .clipped()
                } else {
                    BotanicalFloralWallpaperView()
                        .frame(width: phoneWidth - 10, height: phoneHeight - 10)
                }

                // Camera punch hole
                VStack {
                    Circle()
                        .fill(Color.black)
                        .overlay {
                            Circle().strokeBorder(Color.white.opacity(0.15), lineWidth: 0.6)
                        }
                        .frame(width: 8, height: 8)
                        .padding(.top, 7)
                    Spacer()
                }

                // Phone Screen content
                VStack(spacing: 0) {
                    Spacer(minLength: 28)

                    // Lock Screen Clock (isolated timer)
                    PhoneClockView()
                        .padding(.bottom, 8)

                    Spacer()

                    // Music Card on Phone (INSIDE phone screen at the bottom, matching AirSync)
                    if let session = activeSession {
                        NowPlayingPhoneWidget(session: session)
                            .padding(.horizontal, 10)
                            .padding(.bottom, 8)
                            .transition(.opacity.combined(with: .scale(scale: 0.95)))
                    }

                    // Bottom Controls on Phone Screen: Share/Transfer button + Mirror button (AirSync design)
                    HStack(spacing: 8) {
                        Button {
                            navigator.presentSendFiles()
                        } label: {
                            Image(systemName: "square.and.arrow.up")
                                .font(.system(size: 11, weight: .semibold))
                                .foregroundStyle(controller.connected ? Color.white : Color.white.opacity(0.40))
                                .frame(width: 34, height: 28)
                                .background {
                                    ZStack {
                                        Capsule(style: .continuous)
                                            .fill(Color.black.opacity(0.45))
                                        Capsule(style: .continuous)
                                            .strokeBorder(Color.white.opacity(0.18), lineWidth: 0.8)
                                    }
                                }
                                .clipShape(Capsule(style: .continuous))
                                .shadow(color: Color.black.opacity(0.30), radius: 2.5, x: 0, y: 1)
                        }
                        .buttonStyle(.plain)
                        .disabled(!controller.connected)
                        .help(controller.connected ? "Send Files to Phone" : "Connect phone to send files")

                        Button {
                            if !controller.connected {
                                navigator.presentPairing()
                            } else {
                                NSApp.activate(ignoringOtherApps: true)
                                mirroring.openPhoneScreen()
                            }
                        } label: {
                            HStack(spacing: 5) {
                                Image(systemName: "apps.iphone")
                                    .font(.system(size: 11, weight: .semibold))
                                Text("Mirror")
                                    .font(.system(size: 11.5, weight: .semibold))
                            }
                            .foregroundStyle(Color.white)
                            .padding(.horizontal, 14)
                            .frame(height: 28)
                            .background {
                                ZStack {
                                    Capsule(style: .continuous)
                                        .fill(Color.black.opacity(0.45))
                                    Capsule(style: .continuous)
                                        .strokeBorder(Color.white.opacity(0.18), lineWidth: 0.8)
                                }
                            }
                            .clipShape(Capsule(style: .continuous))
                            .shadow(color: Color.black.opacity(0.30), radius: 2.5, x: 0, y: 1)
                        }
                        .buttonStyle(.plain)
                        .help(controller.connected ? "Open Phone Screen Mirror" : "Phone not connected — click to pair")
                    }
                    .padding(.bottom, 12)
                }

                // Subtle standby dimming overlay when phone is disconnected
                if !controller.connected {
                    Color.black.opacity(0.18)
                        .allowsHitTesting(false)
                }
            }
            .clipShape(RoundedRectangle(cornerRadius: 22, style: .continuous))
            .padding(5)
        }
        .frame(width: phoneWidth, height: phoneHeight)
    }
}

// MARK: - Isolated Clock View (prevents entire window re-renders)

private struct PhoneClockView: View {
    @State private var currentTime = Date()
    private let timer = Timer.publish(every: 1.0, on: .main, in: .common).autoconnect()

    var body: some View {
        VStack(spacing: -12) {
            Text(hoursString)
                .font(.system(size: 58, weight: .regular, design: .rounded))
            Text(minutesString)
                .font(.system(size: 58, weight: .regular, design: .rounded))
        }
        .foregroundStyle(.white)
        .shadow(color: .black.opacity(0.40), radius: 6, x: 0, y: 2)
        .onReceive(timer) { newTime in
            currentTime = newTime
        }
    }

    private var is24Hour: Bool {
        let template = DateFormatter.dateFormat(fromTemplate: "j", options: 0, locale: Locale.current) ?? ""
        return template.contains("H") || template.contains("k")
    }

    private var hoursString: String {
        let formatter = DateFormatter()
        formatter.locale = Locale.current
        formatter.dateFormat = is24Hour ? "HH" : "hh"
        return formatter.string(from: currentTime)
    }

    private var minutesString: String {
        let formatter = DateFormatter()
        formatter.locale = Locale.current
        formatter.dateFormat = "mm"
        return formatter.string(from: currentTime)
    }
}

// MARK: - Now Playing Widget on Phone (AirSync MediaPlayerView Style)

private struct NowPlayingPhoneWidget: View {
    let session: MediaControlsModel.Session
    @Environment(MediaControlsModel.self) private var media
    @Environment(NotificationsModel.self) private var notifications

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            // Title with music note
            HStack(spacing: 6) {
                Image(systemName: "music.note.list")
                    .font(.system(size: 11, weight: .bold))
                    .foregroundStyle(.white)

                Text(session.title)
                    .font(.system(size: 12, weight: .bold))
                    .foregroundStyle(.white)
                    .lineLimit(1)

                Spacer(minLength: 0)
            }

            // Artist line
            if let artist = session.artist, !artist.isEmpty {
                Text(artist)
                    .font(.system(size: 10.5, weight: .medium))
                    .foregroundStyle(.white.opacity(0.85))
                    .lineLimit(1)
            }

            // Controls matching reference: [ |< ] [ ▶ (blue pill) ] [ >| ]
            HStack(spacing: 12) {
                Spacer()

                Button(action: handlePrevious) {
                    Image(systemName: "backward.end.fill")
                        .font(.system(size: 10, weight: .semibold))
                        .foregroundStyle(.white)
                        .frame(width: 32, height: 22)
                        .background(Capsule().fill(Color.white.opacity(0.14)))
                }
                .buttonStyle(.plain)
                .help("Previous Track")

                // Prominent elongated blue play/pause pill
                Button(action: handlePlayPause) {
                    HStack {
                        Image(systemName: session.playing ? "pause.fill" : "play.fill")
                            .font(.system(size: 11, weight: .bold))
                            .foregroundStyle(.white)
                    }
                    .frame(width: 44, height: 24)
                    .background(Capsule().fill(Color(red: 0.0, green: 0.50, blue: 1.0)))
                }
                .buttonStyle(.plain)
                .help(session.playing ? "Pause" : "Play")

                Button(action: handleNext) {
                    Image(systemName: "forward.end.fill")
                        .font(.system(size: 10, weight: .semibold))
                        .foregroundStyle(.white)
                        .frame(width: 32, height: 22)
                        .background(Capsule().fill(Color.white.opacity(0.14)))
                }
                .buttonStyle(.plain)
                .help("Next Track")

                Spacer()
            }
            .padding(.top, 4)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .background {
            ZStack {
                RoundedRectangle(cornerRadius: 18, style: .continuous)
                    .fill(.ultraThinMaterial)
                RoundedRectangle(cornerRadius: 18, style: .continuous)
                    .fill(Color.black.opacity(0.40))
                RoundedRectangle(cornerRadius: 18, style: .continuous)
                    .strokeBorder(Color.white.opacity(0.15), lineWidth: 0.8)
            }
        }
        .clipShape(RoundedRectangle(cornerRadius: 18, style: .continuous))
    }

    private func handlePlayPause() {
        if session.id != -1 {
            media.playPause()
        } else {
            triggerNotificationAction(named: ["play", "pause"])
        }
    }

    private func handleNext() {
        if session.id != -1 {
            media.next()
        } else {
            triggerNotificationAction(named: ["next", "skip"])
        }
    }

    private func handlePrevious() {
        if session.id != -1 {
            media.previous()
        } else {
            triggerNotificationAction(named: ["prev", "back"])
        }
    }

    private func triggerNotificationAction(named keywords: [String]) {
        for n in notifications.notifications {
            for action in n.actions {
                let name = action.name.lowercased()
                if keywords.contains(where: { name.contains($0) }) {
                    notifications.press(action, on: n)
                    return
                }
            }
        }
    }
}

// MARK: - Botanical Floral Wallpaper View

private struct BotanicalFloralWallpaperView: View {
    var body: some View {
        Canvas { context, size in
            let bgRect = CGRect(origin: .zero, size: size)
            let bgGradient = Gradient(colors: [
                Color(red: 0.05, green: 0.07, blue: 0.04),
                Color(red: 0.02, green: 0.03, blue: 0.02)
            ])
            context.fill(Path(bgRect), with: .linearGradient(bgGradient, startPoint: .zero, endPoint: CGPoint(x: 0, y: size.height)))

            let glowRect = CGRect(x: size.width * 0.1, y: size.height * 0.25, width: size.width * 0.8, height: size.height * 0.5)
            context.fill(
                Path(ellipseIn: glowRect),
                with: .radialGradient(
                    Gradient(colors: [Color(red: 0.42, green: 0.55, blue: 0.25).opacity(0.4), .clear]),
                    center: CGPoint(x: size.width * 0.5, y: size.height * 0.5),
                    startRadius: 5,
                    endRadius: size.width * 0.5
                )
            )

            var stemPath = Path()
            stemPath.move(to: CGPoint(x: size.width * 0.5, y: size.height * 0.95))
            stemPath.addCurve(
                to: CGPoint(x: size.width * 0.52, y: size.height * 0.25),
                control1: CGPoint(x: size.width * 0.45, y: size.height * 0.65),
                control2: CGPoint(x: size.width * 0.55, y: size.height * 0.4)
            )
            context.stroke(stemPath, with: .color(Color(red: 0.22, green: 0.36, blue: 0.18)), lineWidth: 4)

            let bellCenters: [(CGPoint, CGFloat, CGFloat)] = [
                (CGPoint(x: size.width * 0.35, y: size.height * 0.48), 24, -0.2),
                (CGPoint(x: size.width * 0.52, y: size.height * 0.42), 28, 0.05),
                (CGPoint(x: size.width * 0.68, y: size.height * 0.49), 25, 0.3),
                (CGPoint(x: size.width * 0.42, y: size.height * 0.60), 30, -0.1),
                (CGPoint(x: size.width * 0.62, y: size.height * 0.59), 29, 0.15),
                (CGPoint(x: size.width * 0.50, y: size.height * 0.70), 32, 0.0)
            ]

            for (center, bellSize, angle) in bellCenters {
                var bell = Path()
                let w = bellSize
                let h = bellSize * 1.25

                context.drawLayer { ctx in
                    ctx.translateBy(x: center.x, y: center.y)
                    ctx.rotate(by: Angle(radians: angle))

                    bell.move(to: CGPoint(x: 0, y: -h * 0.5))
                    bell.addCurve(
                        to: CGPoint(x: w * 0.55, y: h * 0.45),
                        control1: CGPoint(x: w * 0.45, y: -h * 0.15),
                        control2: CGPoint(x: w * 0.6, y: h * 0.2)
                    )
                    bell.addLine(to: CGPoint(x: -w * 0.55, y: h * 0.45))
                    bell.addCurve(
                        to: CGPoint(x: 0, y: -h * 0.5),
                        control1: CGPoint(x: -w * 0.6, y: h * 0.2),
                        control2: CGPoint(x: -w * 0.45, y: -h * 0.15)
                    )
                    bell.closeSubpath()

                    let flowerGradient = Gradient(colors: [
                        Color(red: 0.82, green: 0.90, blue: 0.62),
                        Color(red: 0.62, green: 0.76, blue: 0.42),
                        Color(red: 0.38, green: 0.52, blue: 0.26)
                    ])
                    ctx.fill(
                        bell,
                        with: .linearGradient(
                            flowerGradient,
                            startPoint: CGPoint(x: 0, y: -h * 0.5),
                            endPoint: CGPoint(x: 0, y: h * 0.5)
                        )
                    )

                    var rim = Path()
                    rim.move(to: CGPoint(x: -w * 0.55, y: h * 0.45))
                    rim.addQuadCurve(to: CGPoint(x: 0, y: h * 0.48), control: CGPoint(x: -w * 0.25, y: h * 0.40))
                    rim.addQuadCurve(to: CGPoint(x: w * 0.55, y: h * 0.45), control: CGPoint(x: w * 0.25, y: h * 0.40))
                    ctx.stroke(rim, with: .color(Color(red: 0.88, green: 0.94, blue: 0.72)), lineWidth: 1.5)
                }
            }
        }
    }
}

// MARK: - Right Main Content Pane

private struct MainContentPane: View {
    @Binding var isSidebarVisible: Bool
    @Binding var section: AppSection

    @Environment(\.colorScheme) private var colorScheme
    @Environment(EngineController.self) private var controller
    @Environment(NotificationsModel.self) private var notifications
    @Environment(MessagesModel.self) private var messages
    @Environment(PhoneAppsModel.self) private var phoneApps

    /// The tabs displayed in the center header capsule (including Notifications, Messages, Photos, Apps, Calls, Contacts)
    private let primarySections: [AppSection] = [
        .notifications,
        .messages,
        .photos,
        .apps,
        .calls,
        .contacts
    ]

    var body: some View {
        VStack(spacing: 0) {
            // Header bar - EXACT LayoutMetrics.headerHeight matching sidebar to eliminate layout shifting
            HStack(alignment: .center) {
                // If sidebar is hidden, leave space for standard window buttons, then show toggle button
                if !isSidebarVisible {
                    Spacer()
                        .frame(width: 72)

                    Button {
                        withAnimation(.spring(response: 0.35, dampingFraction: 0.8)) {
                            isSidebarVisible.toggle()
                        }
                    } label: {
                        Image(systemName: "sidebar.left")
                            .font(.system(size: 13, weight: .medium))
                            .foregroundStyle(Color.primary.opacity(0.85))
                            .frame(width: 28, height: 28)
                            .background(
                                RoundedRectangle(cornerRadius: 7, style: .continuous)
                                    .fill(colorScheme == .dark ? Color.white.opacity(0.08) : Color.black.opacity(0.06))
                            )
                    }
                    .buttonStyle(.plain)
                    .help("Show Sidebar")
                    .padding(.trailing, 8)
                }

                // Phone Name and Device Switcher Menu
                DeviceSwitcherMenu()

                Spacer()

                // Center Segmented Toolbar Capsule (All tabs: Notifications, Messages, Photos, Apps, Calls, Contacts + Settings)
                HStack(spacing: 3) {
                    ForEach(primarySections) { s in
                        Button {
                            withAnimation(.spring(response: 0.32, dampingFraction: 0.8)) {
                                section = s
                            }
                        } label: {
                            ZStack(alignment: .topTrailing) {
                                Image(systemName: icon(for: s))
                                    .font(.system(size: 12.5, weight: .medium))
                                    .foregroundStyle(
                                        section == s
                                            ? (colorScheme == .dark ? Color.white : Color.accentColor)
                                            : (colorScheme == .dark ? Color.white.opacity(0.65) : Color.primary.opacity(0.65))
                                    )
                                    .frame(width: 30, height: 30)
                                    .background {
                                        if section == s {
                                            Capsule(style: .continuous)
                                                .fill(
                                                    colorScheme == .dark
                                                        ? Color.white.opacity(0.20)
                                                        : Color.accentColor.opacity(0.12)
                                                )
                                                .overlay {
                                                    Capsule(style: .continuous)
                                                        .strokeBorder(
                                                            colorScheme == .dark
                                                                ? Color.white.opacity(0.15)
                                                                : Color.accentColor.opacity(0.20),
                                                            lineWidth: 0.8
                                                        )
                                                }
                                        }
                                    }

                                let count = badge(for: s)
                                if count > 0 {
                                    Circle()
                                        .fill(Color(red: 0.95, green: 0.33, blue: 0.31))
                                        .frame(width: 7, height: 7)
                                        .offset(x: 2, y: -2)
                                }
                            }
                        }
                        .buttonStyle(.plain)
                        .help(s.title)
                    }

                    // Divider before settings
                    Rectangle()
                        .fill(colorScheme == .dark ? Color.white.opacity(0.18) : Color.black.opacity(0.10))
                        .frame(width: 1, height: 16)
                        .padding(.horizontal, 2)

                    // Settings Tab Button (opens Settings window)
                    Button {
                        SettingsOpener.openSettings()
                    } label: {
                        Image(systemName: "gear")
                            .font(.system(size: 13, weight: .medium))
                            .foregroundStyle(colorScheme == .dark ? Color.white.opacity(0.65) : Color.primary.opacity(0.65))
                            .frame(width: 30, height: 30)
                    }
                    .buttonStyle(.plain)
                    .help("Settings")
                }
                .padding(.horizontal, 5)
                .padding(.vertical, 3)
                .background {
                    ZStack {
                        Capsule(style: .continuous)
                            .fill(.ultraThinMaterial)
                        Capsule(style: .continuous)
                            .fill(colorScheme == .dark ? Color.white.opacity(0.10) : Color.white.opacity(0.70))
                        Capsule(style: .continuous)
                            .strokeBorder(
                                colorScheme == .dark
                                    ? Color.white.opacity(0.14)
                                    : Color.black.opacity(0.08),
                                lineWidth: 0.8
                            )
                    }
                }
                .clipShape(Capsule(style: .continuous))
                .shadow(color: Color.black.opacity(colorScheme == .dark ? 0.20 : 0.04), radius: 4, x: 0, y: 1.5)

                Spacer()

                // Breeze / Action icon to clear all notifications - ONLY on Notifications page!
                if section == .notifications {
                    Button {
                        withAnimation(.spring(response: 0.35, dampingFraction: 0.8)) {
                            notifications.clearAll()
                        }
                    } label: {
                        ZStack(alignment: .topTrailing) {
                            Image(systemName: "wind")
                                .font(.system(size: 14))
                                .foregroundStyle(
                                    notifications.canClearAll
                                        ? (colorScheme == .dark ? Color.white.opacity(0.90) : Color.primary.opacity(0.90))
                                        : (colorScheme == .dark ? Color.white.opacity(0.35) : Color.primary.opacity(0.30))
                                )
                                .frame(width: 28, height: 28)
                                .background {
                                    Circle()
                                        .fill(colorScheme == .dark ? Color.white.opacity(0.10) : Color.black.opacity(0.06))
                                        .overlay {
                                            Circle()
                                                .strokeBorder(colorScheme == .dark ? Color.white.opacity(0.14) : Color.black.opacity(0.08), lineWidth: 0.8)
                                        }
                                }

                            let unpinnedCount = notifications.notifications.filter { !$0.pinned }.count
                            if unpinnedCount > 0 {
                                Text("\(unpinnedCount)")
                                    .font(.system(size: 8.5, weight: .bold, design: .rounded))
                                    .foregroundStyle(.white)
                                    .padding(.horizontal, 4)
                                    .frame(minWidth: 13, minHeight: 13)
                                    .background(Color(red: 0.95, green: 0.33, blue: 0.31), in: Capsule())
                                    .offset(x: 4, y: -4)
                            }
                        }
                    }
                    .buttonStyle(.plain)
                    .help("Clear All Notifications (⌘⌫)")
                    .keyboardShortcut(.delete, modifiers: .command)
                    .disabled(!notifications.canClearAll)
                    .transition(.opacity.combined(with: .scale(scale: 0.85)))
                } else {
                    // Invisible placeholder to keep center capsule perfectly positioned
                    Color.clear
                        .frame(width: 28, height: 28)
                }
            }
            .frame(height: LayoutMetrics.headerHeight)
            .padding(.horizontal, 20)

            // Content Area displaying the selected section
            ZStack {
                switch section {
                case .notifications:
                    NotificationCardsListView()
                case .messages:
                    NavigationSplitView {
                        ConversationListColumn()
                    } detail: {
                        ConversationDetailColumn()
                    }
                case .apps:
                    AppsColumn()
                case .calls:
                    NavigationSplitView {
                        CallListColumn()
                    } detail: {
                        CallDetailColumn()
                    }
                case .contacts:
                    NavigationSplitView {
                        ContactListColumn()
                    } detail: {
                        ContactDetailColumn()
                    }
                case .photos:
                    PhotoGridColumn()
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background {
                ZStack {
                    RoundedRectangle(cornerRadius: 20, style: .continuous)
                        .fill(.ultraThinMaterial)
                    RoundedRectangle(cornerRadius: 20, style: .continuous)
                        .fill(colorScheme == .dark ? Color.black.opacity(0.20) : Color.white.opacity(0.35))
                    RoundedRectangle(cornerRadius: 20, style: .continuous)
                        .strokeBorder(colorScheme == .dark ? Color.white.opacity(0.08) : Color.black.opacity(0.06), lineWidth: 1)
                }
            }
            .clipShape(RoundedRectangle(cornerRadius: 20, style: .continuous))
            .padding(.horizontal, 12)
            .padding(.bottom, 12)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private func icon(for section: AppSection) -> String {
        switch section {
        case .notifications: return "bell.badge"
        case .messages: return "message"
        case .apps: return "square.grid.2x2"
        case .calls: return "phone"
        case .contacts: return "person.crop.circle"
        case .photos: return "photo.on.rectangle"
        }
    }

    private func badge(for section: AppSection) -> Int {
        switch section {
        case .messages: return messages.unreadCount
        case .notifications: return notifications.badgeCount
        case .calls: return 0
        case .contacts, .photos, .apps: return 0
        }
    }
}

// MARK: - Device Switcher Menu

private struct DeviceSwitcherMenu: View {
    @Environment(EngineController.self) private var controller
    @Environment(AppNavigator.self) private var navigator
    @Environment(PhoneFilesModel.self) private var phoneFiles
    @Environment(\.colorScheme) private var colorScheme

    private var isConnecting: Bool {
        switch controller.phase {
        case .starting, .running:
            return !controller.connected && controller.connectFailure == nil
        default:
            return false
        }
    }

    var body: some View {
        Menu {
            // Section: Linked phones
            if !controller.phones.isEmpty {
                Section("Phones") {
                    ForEach(controller.phones, id: \.id) { phone in
                        Button {
                            if phone.id != controller.phone?.id {
                                controller.selectPhone(phone.id)
                            }
                        } label: {
                            HStack {
                                Text(phone.name)
                                if phone.id == controller.phone?.id {
                                    Spacer()
                                    Image(systemName: "checkmark")
                                }
                            }
                        }
                    }
                }
                Divider()
            }

            // Section: Actions
            if !controller.connected {
                Button {
                    controller.refresh()
                } label: {
                    Label("Reconnect Phone", systemImage: "arrow.clockwise")
                }
            }

            Button {
                navigator.presentPairing()
            } label: {
                Label("Pair a New Phone…", systemImage: "qrcode")
            }

            if phoneFiles.enabled {
                Button {
                    Task { await phoneFiles.showInFinder() }
                } label: {
                    Label("Show in Finder", systemImage: "folder")
                }
            }

            Divider()

            Button {
                SettingsOpener.openSettings()
            } label: {
                Label("Settings…", systemImage: "gearshape")
            }
        } label: {
            HStack(spacing: 5) {
                // Connection indicator dot
                Circle()
                    .fill(controller.connected ? Color.green : Color.orange)
                    .frame(width: 6, height: 6)

                Image(systemName: "iphone")
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundStyle(Color.accentColor)

                Text(controller.phone?.name ?? (controller.phones.first?.name ?? String(localized: "No Phone")))
                    .font(.system(size: 13.5, weight: .bold))
                    .foregroundStyle(Color.primary)
                    .lineLimit(1)

                if !controller.connected {
                    Text(isConnecting ? "(Connecting…)" : "(Offline)")
                        .font(.system(size: 11, weight: .regular))
                        .foregroundStyle(Color.secondary)
                }

                Image(systemName: "chevron.down")
                    .font(.system(size: 8, weight: .bold))
                    .foregroundStyle(Color.secondary)
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 5)
            .background {
                ZStack {
                    Capsule(style: .continuous)
                        .fill(.ultraThinMaterial)
                    Capsule(style: .continuous)
                        .fill(colorScheme == .dark ? Color.white.opacity(0.08) : Color.white.opacity(0.65))
                    Capsule(style: .continuous)
                        .strokeBorder(
                            colorScheme == .dark
                                ? Color.white.opacity(0.12)
                                : Color.black.opacity(0.08),
                            lineWidth: 0.8
                        )
                }
            }
            .clipShape(Capsule(style: .continuous))
            .shadow(color: Color.black.opacity(colorScheme == .dark ? 0.20 : 0.04), radius: 3, x: 0, y: 1)
            .contentShape(Capsule(style: .continuous))
        }
        .menuStyle(.button)
        .buttonStyle(.plain)
        .menuIndicator(.hidden)
        .help("Switch device or pair a new phone")
    }
}

// MARK: - Reliable Settings Opener

@MainActor
private enum SettingsOpener {
    static func openSettings() {
        NSApp.activate(ignoringOtherApps: true)
        if let appMenu = NSApp.mainMenu?.items.first?.submenu,
           let index = appMenu.items.firstIndex(where: { $0.keyEquivalent == "," }) {
            appMenu.performActionForItem(at: index)
        } else {
            NSApp.sendAction(Selector(("showSettingsWindow:")), to: nil, from: nil)
        }
    }
}

// MARK: - Notification Cards List

private struct NotificationCardsListView: View {
    @Environment(NotificationsModel.self) private var model
    @Environment(\.undoManager) private var undoManager

    var body: some View {
        ScrollView(.vertical, showsIndicators: true) {
            LazyVStack(spacing: 12) {
                if !model.notifications.isEmpty {
                    // Render live notifications synced from the phone
                    ForEach(model.notifications, id: \.key) { notification in
                        AppleGlassNotificationCard(
                            notification: notification,
                            onDismiss: {
                                model.dismiss(notification)
                            }
                        )
                        .contextMenu {
                            NotificationRowContextMenu(notification: notification, undoManager: undoManager)
                        }
                    }
                } else {
                    // Empty state (matching AirSync NotificationEmptyView)
                    VStack(spacing: 12) {
                        Image(systemName: "bell.slash")
                            .font(.system(size: 38, weight: .light))
                            .foregroundStyle(Color.secondary.opacity(0.7))
                            .padding(.bottom, 4)

                        Text("No Notifications")
                            .font(.system(size: 16, weight: .bold))
                            .foregroundStyle(Color.primary)

                        Text("New notifications from your phone appear here.")
                            .font(.system(size: 13))
                            .foregroundStyle(Color.secondary)
                    }
                    .padding(40)
                    .frame(maxWidth: .infinity, minHeight: 300)
                }
            }
            .padding(.top, 16) // Required breathing room margin on first notification!
            .padding(.horizontal, 20)
            .padding(.bottom, 24)
        }
    }
}

// MARK: - Apple Glass Notification Card with Working Inline Reply

private struct AppleGlassNotificationCard: View {
    let notification: PhoneNotification
    var onDismiss: (() -> Void)? = nil

    @State private var isHovered = false
    @State private var isReplying = false
    @State private var replyText = ""
    @FocusState private var replyFocused: Bool

    @Environment(\.colorScheme) private var colorScheme
    @Environment(NotificationsModel.self) private var model

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .top, spacing: 12) {
                // Vibrant App Icon with Fallback Monogram
                NotificationAppIconView(
                    packageName: notification.packageName,
                    appName: NotificationsModel.appName(of: notification),
                    size: 28
                )
                .frame(width: 28, height: 28)
                .clipShape(RoundedRectangle(cornerRadius: 7, style: .continuous))
                .padding(.top, 2)

                // Title and Body Content
                VStack(alignment: .leading, spacing: 4) {
                    Text("\(NotificationsModel.appName(of: notification)) - \(notification.title ?? "")")
                        .font(.system(size: 13.5, weight: .bold))
                        .foregroundStyle(Color.primary)
                        .lineLimit(1)

                    let preview = NotificationsModel.preview(of: notification)
                    Text(preview.isEmpty ? notification.body : preview)
                        .font(.system(size: 12.5, weight: .regular))
                        .foregroundStyle(Color.primary.opacity(0.85))
                        .lineSpacing(3)
                        .fixedSize(horizontal: false, vertical: true)
                }

                Spacer(minLength: 8)

                // Dismiss Button
                VStack(spacing: 3) {
                    Button {
                        onDismiss?()
                    } label: {
                        ZStack {
                            Circle()
                                .fill(Color(red: 0.95, green: 0.33, blue: 0.31))
                                .frame(width: 28, height: 28)

                            Image(systemName: "trash.fill")
                                .font(.system(size: 11, weight: .semibold))
                                .foregroundStyle(.white)
                        }
                    }
                    .buttonStyle(.plain)

                    Text("Dismiss")
                        .font(.system(size: 9, weight: .medium))
                        .foregroundStyle(Color.secondary)
                }
                .padding(.leading, 4)
            }

            // Inline Reply and Action Buttons Row (Matching AirSync ReplyActionButton!)
            if notification.reply != nil || !notification.actions.isEmpty {
                HStack(spacing: 8) {
                    if isReplying {
                        // Expanded Inline Reply Field + Send Button
                        HStack(spacing: 6) {
                            TextField(notification.reply?.name ?? "Reply…", text: $replyText)
                                .textFieldStyle(.plain)
                                .font(.system(size: 12))
                                .foregroundStyle(Color.primary)
                                .focused($replyFocused)
                                .onSubmit(sendReply)
                                .frame(minWidth: 160)

                            Button(action: sendReply) {
                                ZStack {
                                    Circle()
                                        .fill(Color(red: 0.0, green: 0.48, blue: 1.0))
                                        .frame(width: 24, height: 24)

                                    Image(systemName: "paperplane.fill")
                                        .font(.system(size: 10, weight: .semibold))
                                        .foregroundStyle(.white)
                                }
                            }
                            .buttonStyle(.plain)
                            .disabled(!NotificationsModel.canSend(replyText))
                        }
                        .padding(.horizontal, 10)
                        .padding(.vertical, 5)
                        .background {
                            Capsule(style: .continuous)
                                .fill(colorScheme == .dark ? Color.black.opacity(0.40) : Color.white)
                                .overlay {
                                    Capsule(style: .continuous)
                                        .strokeBorder(Color.blue.opacity(0.6), lineWidth: 1)
                                }
                        }
                    } else if notification.reply != nil {
                        // Collapsed Reply Button
                        Button {
                            withAnimation(.spring(response: 0.3, dampingFraction: 0.8)) {
                                isReplying = true
                                replyFocused = true
                            }
                        } label: {
                            HStack(spacing: 4) {
                                Image(systemName: "arrowshape.turn.up.left")
                                    .font(.system(size: 10, weight: .semibold))
                                Text(notification.reply?.name ?? "Reply")
                                    .font(.system(size: 11, weight: .medium))
                            }
                            .foregroundStyle(Color.primary)
                            .padding(.horizontal, 10)
                            .padding(.vertical, 5)
                            .background {
                                Capsule(style: .continuous)
                                    .fill(colorScheme == .dark ? Color.white.opacity(0.12) : Color.black.opacity(0.06))
                                    .overlay {
                                        Capsule(style: .continuous)
                                            .strokeBorder(colorScheme == .dark ? Color.white.opacity(0.14) : Color.black.opacity(0.08), lineWidth: 0.8)
                                    }
                            }
                        }
                        .buttonStyle(.plain)
                    }

                    // Notification's extra action buttons (Mark as read, Mute, etc.)
                    ForEach(notification.actions, id: \.position) { action in
                        Button {
                            model.press(action, on: notification)
                        } label: {
                            Text(action.name)
                                .font(.system(size: 11, weight: .medium))
                                .foregroundStyle(Color.primary.opacity(0.85))
                                .padding(.horizontal, 10)
                                .padding(.vertical, 5)
                                .background {
                                    Capsule(style: .continuous)
                                        .fill(colorScheme == .dark ? Color.white.opacity(0.10) : Color.black.opacity(0.06))
                                        .overlay {
                                            Capsule(style: .continuous)
                                                .strokeBorder(colorScheme == .dark ? Color.white.opacity(0.14) : Color.black.opacity(0.08), lineWidth: 0.8)
                                        }
                                }
                        }
                        .buttonStyle(.plain)
                    }

                    Spacer()

                    if isReplying {
                        Button("Cancel") {
                            withAnimation {
                                isReplying = false
                                replyText = ""
                            }
                        }
                        .font(.system(size: 11))
                        .foregroundStyle(Color.secondary)
                        .buttonStyle(.plain)
                    }
                }
                .padding(.top, 2)
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 12)
        .background {
            ZStack {
                RoundedRectangle(cornerRadius: 18, style: .continuous)
                    .fill(.ultraThinMaterial)
                RoundedRectangle(cornerRadius: 18, style: .continuous)
                    .fill(
                        colorScheme == .dark
                            ? Color(red: 0.14, green: 0.13, blue: 0.12).opacity(isHovered ? 0.85 : 0.70)
                            : Color.white.opacity(isHovered ? 0.80 : 0.65)
                    )
                RoundedRectangle(cornerRadius: 18, style: .continuous)
                    .strokeBorder(
                        colorScheme == .dark
                            ? (isHovered ? Color.white.opacity(0.22) : Color.white.opacity(0.10))
                            : (isHovered ? Color.black.opacity(0.12) : Color.black.opacity(0.06)),
                        lineWidth: 0.8
                    )
            }
        }
        .clipShape(RoundedRectangle(cornerRadius: 18, style: .continuous))
        .shadow(
            color: Color.black.opacity(colorScheme == .dark ? 0.20 : 0.06),
            radius: isHovered ? 8 : 4,
            x: 0,
            y: 2
        )
        .onHover { isHovered = $0 }
    }

    private func sendReply() {
        guard NotificationsModel.canSend(replyText) else { return }
        if model.sendReply(replyText, to: notification) {
            replyText = ""
            withAnimation {
                isReplying = false
            }
        }
    }
}

// MARK: - Notification App Icon with Crisp Monogram Fallback

private struct NotificationAppIconView: View {
    let packageName: String
    let appName: String
    var size: CGFloat = 28

    @Environment(PhoneAppsModel.self) private var phoneApps

    var body: some View {
        if let image = phoneApps.icon(for: packageName) {
            Image(nsImage: image)
                .resizable()
                .scaledToFit()
        } else {
            // High-fidelity fallback badge based on app identity
            ZStack {
                fallbackColor
                Text(fallbackGlyph)
                    .font(.system(size: size * 0.45, weight: .bold))
                    .foregroundStyle(.white)
            }
        }
    }

    private var lowerPkg: String { packageName.lowercased() }
    private var lowerName: String { appName.lowercased() }

    private var fallbackColor: Color {
        if lowerPkg.contains("whatsapp") || lowerName.contains("whatsapp") {
            return Color(red: 0.15, green: 0.75, blue: 0.38)
        } else if lowerPkg.contains("facebook") || lowerName.contains("facebook") {
            return Color(red: 0.11, green: 0.45, blue: 0.92)
        } else if lowerPkg.contains("github") || lowerName.contains("github") {
            return Color(red: 0.15, green: 0.15, blue: 0.17)
        } else if lowerPkg.contains("gmail") || lowerName.contains("gmail") {
            return Color(red: 0.90, green: 0.25, blue: 0.20)
        } else if lowerPkg.contains("message") || lowerPkg.contains("sms") {
            return Color(red: 0.0, green: 0.48, blue: 1.0)
        } else if lowerPkg.contains("telegram") {
            return Color(red: 0.18, green: 0.65, blue: 0.95)
        } else {
            return Color(red: 0.30, green: 0.35, blue: 0.45)
        }
    }

    private var fallbackGlyph: String {
        if lowerPkg.contains("whatsapp") || lowerName.contains("whatsapp") {
            return "W"
        } else if lowerPkg.contains("facebook") || lowerName.contains("facebook") {
            return "f"
        } else if lowerPkg.contains("gmail") || lowerName.contains("gmail") {
            return "M"
        } else if let first = appName.first {
            return String(first).uppercased()
        } else {
            return "A"
        }
    }
}

// MARK: - Row Context Menu

private struct NotificationRowContextMenu: View {
    let notification: PhoneNotification
    let undoManager: UndoManager?
    @Environment(NotificationsModel.self) private var model
    @Environment(ScreenMirroringModel.self) private var mirroring

    var body: some View {
        let app = NotificationsModel.appName(of: notification)
        if NotificationsModel.opensInMessages(notification) {
            Button("Open in Messages") { model.openInMessages(notification) }
        }
        if mirroring.canOpen(notification) {
            Button("Open on Phone") { mirroring.open(notification) }
        }
        if notification.reply != nil {
            Button("Reply…") { model.beginReply(to: notification) }
        }
        ForEach(notification.actions, id: \.position) { action in
            Button(action.name) { model.press(action, on: notification) }
        }
        Button(notification.pinned ? "Unpin" : "Pin") {
            model.setPinned(notification, !notification.pinned)
        }

        if model.appCommandsAvailable {
            Divider()
            if model.isAppOn(notification) {
                Button("Turn Off Notifications from \(app)") {
                    model.turnOffApp(of: notification, undoManager: undoManager)
                }
            } else {
                Button("Turn On Notifications from \(app)") { model.turnOnApp(of: notification) }
            }
        }

        Divider()
        Button("Dismiss", role: .destructive) { model.dismiss(notification) }
    }
}

// MARK: - AppKit Visual Effect View Wrapper

struct VisualEffectView: NSViewRepresentable {
    var material: NSVisualEffectView.Material = .hudWindow
    var blendingMode: NSVisualEffectView.BlendingMode = .behindWindow
    var state: NSVisualEffectView.State = .active

    func makeNSView(context: Context) -> NSVisualEffectView {
        let view = NSVisualEffectView()
        view.material = material
        view.blendingMode = blendingMode
        view.state = state
        return view
    }

    func updateNSView(_ nsView: NSVisualEffectView, context: Context) {
        nsView.material = material
        nsView.blendingMode = blendingMode
        nsView.state = state
    }
}

extension View {
    /// Hides the window toolbar where the API exists (macOS 15+); a no-op on earlier macOS.
    @ViewBuilder
    func hideWindowToolbar() -> some View {
        if #available(macOS 15, *) {
            self.toolbarVisibility(.hidden, for: .windowToolbar)
        } else {
            self
        }
    }
}
