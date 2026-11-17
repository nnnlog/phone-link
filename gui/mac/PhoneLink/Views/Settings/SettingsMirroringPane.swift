import PhoneLinkEngine
import PhoneLinkKit
import SwiftUI

/// Phone Screen: whether the phone's screen and apps open on this Mac, how the phone behaves
/// meanwhile, its audio, and the questions the windows ask before acting.
struct SettingsMirroringPane: View {
    @Environment(ScreenMirroringModel.self) private var model

    var body: some View {
        Form {
            if let status = model.status {
                if model.isOffered {
                    sections(status)
                } else {
                    Section {
                        Text("This phone's screen can't be shown on this Mac.")
                            .foregroundStyle(.secondary)
                    }
                }
            } else {
                SettingsUnavailableNote()
            }
        }
        .settingsPaneForm()
        .onAppear { model.reload() }
    }

    @ViewBuilder
    private func sections(_ status: MirroringStatusView) -> some View {
        let settings = status.settings
        Section {
            Toggle(isOn: binding(\.allowApps)) {
                Text("Open your phone's screen and apps on this Mac")
                Text("Turning this off closes their windows")
            }
            Toggle(isOn: binding(\.blackScreen)) {
                Text("Turn your phone's screen black while it's shown here")
            }
            .disabled(!settings.allowApps)
            if status.audioSupported {
                Toggle(isOn: binding(\.audioStreaming)) {
                    Text("Play apps' audio on this Mac")
                }
                .disabled(!settings.allowApps)
            }
            if status.adaptiveLayout {
                Toggle(isOn: binding(\.alwaysExpanded)) {
                    Text("Always open apps in the expanded layout")
                }
                .disabled(!settings.allowApps)
            }
        }
        Section("Ask Before Continuing") {
            Toggle("When your phone moves an app to this Mac", isOn: asking(\.doNotAskRestart))
            Toggle("When your phone hands off what you're doing to this Mac", isOn: asking(\.doNotAskTaskTransfer))
            Toggle("When an app checks that you're touching your phone", isOn: asking(\.doNotShowTrueTouch))
            if status.adaptiveLayout {
                Toggle("When you change an app's layout", isOn: asking(\.doNotAskSwitchLayout))
            }
            Toggle("When connecting without unlocking your phone", isOn: asking(\.doNotAskSkipUnlock))
            Toggle("When your phone has been verified with a code", isOn: asking(\.doNotAskSkipVerification))
        }
        .disabled(!settings.allowApps)
    }

    private func binding(_ key: WritableKeyPath<MirrorSettings, Bool>) -> Binding<Bool> {
        Binding(
            get: { model.status?.settings[keyPath: key] ?? false },
            set: { value in model.updateSettings { $0[keyPath: key] = value } }
        )
    }

    /// A "do not ask" flag shown as its question being asked.
    private func asking(_ key: WritableKeyPath<MirrorSettings, Bool>) -> Binding<Bool> {
        Binding(
            get: { !(model.status?.settings[keyPath: key] ?? false) },
            set: { value in model.updateSettings { $0[keyPath: key] = !value } }
        )
    }
}
