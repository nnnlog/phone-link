import PhoneLinkEngine
import PhoneLinkKit
import SwiftUI

private let log = AppLog(category: "settings")

/// Settings › Advanced: this Mac's device-bound registration with the Microsoft account, a reset
/// that re-registers it on the next sign-in, and the app's log.
struct SettingsAdvancedPane: View {
    @Environment(EngineController.self) private var controller
    @State private var registration: DeviceRegistration?
    @State private var confirmingReset = false
    @State private var resetting = false
    @State private var failure: String?

    var body: some View {
        Form {
            Section {
                if let registration {
                    LabeledContent("Device ID") {
                        Text(registration.puid).textSelection(.enabled).monospaced()
                    }
                    if registration.distinguishedDeviceIds.isEmpty {
                        LabeledContent("Linked ID", value: String(localized: "Not yet listed"))
                    } else {
                        ForEach(Array(registration.distinguishedDeviceIds.enumerated()), id: \.offset) { _, id in
                            LabeledContent("Linked ID") {
                                Text(id).textSelection(.enabled).monospaced()
                            }
                        }
                    }
                } else {
                    Text("This Mac is not registered yet.").foregroundStyle(.secondary)
                }
            } header: {
                Text("Device Registration")
            } footer: {
                Text(
                    "Phone Link registers this Mac with your Microsoft account as a Windows device so your phone can reach it."
                )
            }

            Section {
                Button("Reset Device Registration…", role: .destructive) { confirmingReset = true }
                    .disabled(!controller.canSignOut || resetting)
            } footer: {
                Text(
                    "Removes this Mac's records from your account and signs out. The next sign-in registers this Mac again with a new ID. The earlier registration stays on your Microsoft account."
                )
            }

            SettingsLogSection()
        }
        .settingsPaneForm()
        .task { registration = controller.deviceRegistration() }
        .confirmationDialog(
            "Reset This Mac's Device Registration?",
            isPresented: $confirmingReset,
            titleVisibility: .visible
        ) {
            Button("Reset", role: .destructive) { reset() }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("This Mac will be removed from your account and signed out. Sign in again to reconnect your phone.")
        }
        .alert("Phone Link Couldn't Reset the Registration", isPresented: failurePresented) {
        } message: {
            Text(failure ?? "")
        }
    }

    private var failurePresented: Binding<Bool> {
        Binding(get: { failure != nil }, set: { if !$0 { failure = nil } })
    }

    private func reset() {
        resetting = true
        Task {
            do {
                try await controller.resetDeviceRegistration()
            } catch {
                log.error("device registration not reset: \(error: error)")
                failure = UserText.describe(error)
            }

            resetting = false
            registration = controller.deviceRegistration()
        }
    }
}
