import PhoneLinkKit
import SwiftUI

/// Notifications: whether the phone's notifications show on this Mac, how each app's do, and the
/// access the phone gives.
struct SettingsNotificationsPane: View {
    @Environment(SettingsModel.self) private var model
    @Environment(EngineController.self) private var controller

    var body: some View {
        @Bindable var model = model
        Form {
            if !model.isAvailable {
                SettingsUnavailableNote()
            }
            Section {
                Toggle("Show notifications from my phone", isOn: $model.showsPhoneNotifications)
                Toggle("Alert me when the phone's battery is low", isOn: $model.alertsLowBattery)
                Toggle("Pause apps that send too many notifications", isOn: $model.pausesNoisyApps)
                    .disabled(!model.showsPhoneNotifications)
            }
            .disabled(!model.isAvailable)
            if let state = model.notificationsState {
                // Per-app settings apply only while the phone reports its apps.
                if state.appListSupported {
                    Section {
                        ForEach(model.notificationApps) { app in
                            AppRow(app: app)
                        }
                    } header: {
                        Text("Apps")
                    } footer: {
                        Text("Apps appear here after they send a notification. Choosing Off for an app also stops your phone from sending its notifications.")
                    }
                    .disabled(!state.pcPermission)
                }
                if state.capable {
                    Section("On \(controller.phone?.name ?? String(localized: "Your Phone"))") {
                        if state.lowRamDevice && !state.devicePermission {
                            LabeledContent("Notification access", value: String(localized: "Not Supported"))
                        } else {
                            SettingsAccessRow(
                                title: "Notification access",
                                allowed: state.devicePermission,
                                pending: model.pendingRequests.contains(.notificationAccess)
                            ) {
                                Task { await model.requestNotificationAccess() }
                            }
                        }
                        if state.sensitivePromptOffered {
                            SettingsAccessRow(
                                title: "Sensitive content",
                                allowed: state.sensitivePermission,
                                pending: model.pendingRequests.contains(.sensitiveContent)
                            ) {
                                Task { await model.requestSensitiveContent() }
                            }
                        }
                    }
                }
            }
        }
        .settingsPaneForm()
        .onAppear {
            model.setNotificationsPaneVisible(true)
            model.reload()
        }
        .onDisappear { model.setNotificationsPaneVisible(false) }
        // Again once the engine starts while the pane is open.
        .task(id: model.isAvailable) { await model.notificationsPaneAppeared() }
    }
}

/// An app's pop-up: how its notifications reach this Mac.
private struct AppRow: View {
    let app: SettingsNotificationApp
    @Environment(SettingsModel.self) private var model

    var body: some View {
        Picker(selection: choice) {
            ForEach(SettingsNotificationChoice.allCases, id: \.self) { choice in
                Text(choice.title).tag(choice)
            }
        } label: {
            Text(app.name)
            if let muted = app.mutedText {
                Text(muted)
            }
        }
        .pickerStyle(.menu)
        .disabled(model.pendingApps.contains(app.packageName))
    }

    private var choice: Binding<SettingsNotificationChoice> {
        Binding(
            get: { app.choice },
            set: { choice in
                Task { await model.setChoice(choice, forApp: app.packageName) }
            }
        )
    }
}
