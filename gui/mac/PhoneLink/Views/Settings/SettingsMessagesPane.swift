import PhoneLinkKit
import SwiftUI

/// Messages: whether the phone's messages show on this Mac, and the phone's contacts that name them
/// (call history is in the Calls pane).
struct SettingsMessagesPane: View {
    @Environment(SettingsModel.self) private var model

    var body: some View {
        @Bindable var model = model
        Form {
            if !model.isAvailable {
                SettingsUnavailableNote()
            }
            Section {
                Toggle("Show messages on this Mac", isOn: $model.showsMessages)
                Toggle("Notify me about new messages", isOn: $model.notifiesMessages)
                    .disabled(!model.showsMessages)
                Toggle("Download pictures automatically", isOn: $model.downloadsPictures)
            }
            .disabled(!model.isAvailable)
            if model.contactsState?.capable != false {
                Section("Contacts") {
                    Toggle("Use my phone's contacts", isOn: $model.usesPhoneContacts)
                    // While the Mac does not use them the phone's answer is not known.
                    if let contacts = model.contactsState, contacts.access != .pcConsentRequired {
                        SettingsAccessRow(
                            title: "Contacts access",
                            allowed: contacts.access == .granted,
                            pending: model.pendingRequests.contains(.contactsAccess)
                        ) {
                            Task { await model.requestContactsAccess() }
                        }
                    }
                }
                .disabled(!model.isAvailable)
            }
        }
        .settingsPaneForm()
        .onAppear { model.reload() }
    }
}
