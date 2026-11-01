import AppKit
import PhoneLinkKit
import SwiftUI

/// The menu bar extra's menu: the phone's status, its controls in the order of the main window's
/// phone area, what plays on the phone, and the app's own commands. It is a standard menu (HIG › Menu
/// bar extras), so its status lines are disabled items, and only the status lines and the controls
/// carry symbols.
struct PhoneMenu: View {
    @Environment(EngineController.self) private var controller
    @Environment(AppNavigator.self) private var navigator
    @Environment(UpdateModel.self) private var updates
    @Environment(\.openSettings) private var openSettings

    var body: some View {
        switch controller.phase {
        case .running:
            if controller.phoneUnlinked {
                Text("Phone Not Linked")
                Button("Try Again") { controller.start() }
            } else {
                PhoneStatusItems()
            }
        case .idle, .starting:
            Text("Connecting…")
        case .webSignIn:
            Text("Sign In to Continue")
            Button("Sign In…") { navigator.showSignIn() }
        case .signedOut:
            Text("Signed Out")
            Button("Sign In…") { controller.start() }
        case .selectingPhone:
            Text("Choose a Phone")
            Button("Choose Phone…") { navigator.showSignIn() }
        case .failed:
            Text("Couldn't Connect")
            Button("Try Again") { controller.start() }
        case .accountRemoved:
            Text("Sign In to Continue")
            Button("Sign In…") { controller.start() }
        }
        Divider()
        Button("Open Phone Link") { navigator.showWindow() }
        Button("Settings…") {
            // An app without a Dock icon is not active while its menu is open; without this the
            // window opens behind the app in front.
            NSApp.activate()
            openSettings()
        }
        .keyboardShortcut(",")
        if updates.isEnabled {
            if let release = updates.offered {
                Button("Update to Phone Link \(release.version.description)…") { updates.showWindow() }
            } else {
                Button("Check for Updates…") { updates.checkNow() }
            }
        }
        Divider()
        Button("About Phone Link") { AboutPanel.show() }
        Button("Quit Phone Link") { NSApp.terminate(nil) }
            .keyboardShortcut("q")
    }
}

/// The phone's name and connection, its status lines, and its controls.
///
/// The name is a plain disabled item rather than a `Section` header: SwiftUI brackets a menu section
/// with separators of its own, which would add a line above the menu's first item and a second one
/// below the status.
private struct PhoneStatusItems: View {
    @Environment(EngineController.self) private var controller
    @Environment(DeviceStatusModel.self) private var deviceStatus
    @Environment(HotspotModel.self) private var hotspot
    @Environment(MediaControlsModel.self) private var media
    @Environment(FileTransferModel.self) private var files
    @Environment(AppNavigator.self) private var navigator
    @Environment(ScreenMirroringModel.self) private var mirroring

    var body: some View {
        Text(header)
        if !controller.connected, !controller.allFeaturesDisabled, let failure = controller.connectFailure {
            Text(failure)
        }
        let lines = deviceStatus.lines
        if lines.isEmpty {
            Text("Phone Status Unavailable")
        } else {
            ForEach(lines) { line in
                Label {
                    Text(line.text)
                } icon: {
                    if let value = line.variableValue {
                        Image(systemName: line.symbol, variableValue: value)
                    } else {
                        Image(systemName: line.symbol)
                    }
                }
            }
        }
        PhoneControlGroups(
            deviceStatus: deviceStatus,
            hotspot: hotspot,
            files: files,
            navigator: navigator,
            mirroring: mirroring,
            media: media,
            separatesFirstGroup: true
        )
    }

    private var header: String {
        let name = controller.phone?.name ?? String(localized: "Phone")
        return if controller.allFeaturesDisabled {
            String(localized: "\(name) · Link to Windows Turned Off")
        } else if controller.connected {
            String(localized: "\(name) · Connected")
        } else {
            String(localized: "\(name) · Not Connected")
        }
    }
}

/// The phone's controls, in the menu bar extra and in the main menu's Phone menu, grouped and ordered
/// as in the main window's phone area: Do Not Disturb and the ringer; the hotspot and ring-my-phone;
/// sending files and the phone's screen; then what plays on the phone. A group shows only what the
/// phone offers, and a separator sets it off from the group above — and from the items above the
/// first group when `separatesFirstGroup`.
struct PhoneControlGroups: View {
    let deviceStatus: DeviceStatusModel
    let hotspot: HotspotModel
    let files: FileTransferModel
    let navigator: AppNavigator
    let mirroring: ScreenMirroringModel
    let media: MediaControlsModel
    let separatesFirstGroup: Bool

    var body: some View {
        let modes = deviceStatus.doNotDisturb != nil || deviceStatus.ringer != nil
        let reach = hotspot.item != nil || deviceStatus.ringMyPhone != nil
        let handoff = files.canSend || mirroring.isOffered

        if modes {
            if separatesFirstGroup {
                Divider()
            }
            PhoneModeItems(deviceStatus: deviceStatus)
        }
        if reach {
            if separatesFirstGroup || modes {
                Divider()
            }
            HotspotItem(hotspot: hotspot)
            RingPhoneItems(deviceStatus: deviceStatus)
        }
        if handoff {
            if separatesFirstGroup || modes || reach {
                Divider()
            }
            SendFilesItem(files: files, navigator: navigator)
            PhoneScreenItem(mirroring: mirroring)
        }

        // Media opens with its own separator.
        MediaControlItems(media: media)
    }

    /// Any group shows.
    var showsAnything: Bool {
        deviceStatus.doNotDisturb != nil || deviceStatus.ringer != nil || hotspot.item != nil
            || deviceStatus.ringMyPhone != nil || files.canSend || mirroring.isOffered || (media.isShown && media.selected != nil)
    }
}

/// The phone's Do Not Disturb and ringer controls; nothing while the phone offers neither.
private struct PhoneModeItems: View {
    let deviceStatus: DeviceStatusModel

    var body: some View {
        if let dnd = deviceStatus.doNotDisturb {
            if let isOn = dnd.isOn {
                Toggle(isOn: Binding(get: { isOn }, set: { _ in deviceStatus.toggleDoNotDisturb() })) {
                    Label(dnd.title, systemImage: "moon")
                }
                .disabled(!dnd.isEnabled)
            } else {
                Label(dnd.title, systemImage: "moon")
            }
        }
        if let ringer = deviceStatus.ringer {
            // A label followed by a text: SwiftUI makes the text the menu item's second line.
            Button {
                deviceStatus.cycleRingerMode()
            } label: {
                Label(ringer.title, systemImage: ringer.symbol)
                if let hint = ringer.hint {
                    Text(hint)
                }
            }
            .disabled(!ringer.isEnabled)
        }
    }
}

/// Ring the phone to find it, or — while it rings — the seconds left and Stop; nothing while the
/// phone doesn't offer it.
private struct RingPhoneItems: View {
    let deviceStatus: DeviceStatusModel

    var body: some View {
        if let ring = deviceStatus.ringMyPhone {
            if ring.isRinging {
                // A line, not a command: the seconds left.
                Label(ring.title, systemImage: ring.symbol)
                if let stop = ring.stopTitle {
                    Button {
                        deviceStatus.stopRinging()
                    } label: {
                        Label(stop, systemImage: "bell.slash")
                    }
                }
            } else {
                Button {
                    deviceStatus.ringPhone()
                } label: {
                    Label(ring.title, systemImage: ring.symbol)
                }
                .disabled(!ring.isEnabled)
            }
        }
    }
}

/// Open the phone's screen in a window; nothing while mirroring is not offered.
private struct PhoneScreenItem: View {
    let mirroring: ScreenMirroringModel

    var body: some View {
        if mirroring.isOffered {
            Button {
                NSApp.activate()
                mirroring.openPhoneScreen()
            } label: {
                Label("Open Phone Screen", systemImage: "iphone")
            }
        }
    }
}

/// Send files to the phone (the send window); nothing while the phone is not offered as a target.
private struct SendFilesItem: View {
    let files: FileTransferModel
    let navigator: AppNavigator

    var body: some View {
        if files.canSend {
            Button {
                navigator.showSendFiles()
            } label: {
                Label("Send Files to Phone…", systemImage: "arrow.up.doc")
            }
        }
    }
}

/// Instant hotspot: a toggle, on while this Mac is on the phone's hotspot, that joins it (the first
/// time, pairing with the phone and asking for its consent on the way) or leaves it; nothing while the
/// phone cannot share its connection. macOS's Wi-Fi menu cannot list the phone's hotspot, so it is
/// offered here.
private struct HotspotItem: View {
    let hotspot: HotspotModel

    var body: some View {
        if let item = hotspot.item {
            switch item {
            case .connect, .connected:
                Toggle(isOn: Binding(get: { item == .connected }, set: { _ in hotspot.toggle() })) {
                    Label("Phone Hotspot", systemImage: "personalhotspot")
                }
            case let .working(work):
                Label(work.text, systemImage: "personalhotspot")
            }
        }
    }
}
