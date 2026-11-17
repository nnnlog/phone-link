import PhoneLinkEngine
import PhoneLinkKit
import SwiftUI

private let log = AppLog(category: "settings")

/// Account: the signed-in Microsoft account and signing out, the account's linked phones (which one
/// this Mac uses, and pairing another), and the account's PCs.
struct SettingsAccountPane: View {
    @Environment(SettingsModel.self) private var model
    @Environment(EngineController.self) private var controller
    @Environment(AppNavigator.self) private var navigator
    /// The account's sign-in name (its e-mail address or phone number); read when the pane appears and
    /// after signing out.
    @State private var accountName: String?
    @State private var confirmingSignOut = false
    @State private var signOutFailure: String?

    var body: some View {
        Form {
            if !model.isAvailable {
                SettingsUnavailableNote()
            }
            Section("Microsoft Account") {
                LabeledContent {
                    Button("Sign Out…") { confirmingSignOut = true }
                        .disabled(!controller.canSignOut)
                } label: {
                    Text(accountName ?? String(localized: "Microsoft account"))
                    Text(controller.phase == .signedOut ? "Signed out" : "Signed in")
                }
                .alert("Sign Out of Phone Link?", isPresented: $confirmingSignOut) {
                    Button("Sign Out", role: .destructive) { signOut() }
                    Button("Cancel", role: .cancel) {}
                } message: {
                    Text("This Mac will stop connecting to your phone, and its copies of your messages, photos, contacts and notifications will be deleted. Your phone stays linked to your Microsoft account.")
                }
            }
            Section("Phones") {
                ForEach(controller.phones, id: \.id) { phone in
                    LabeledContent {
                        if phone.id == selectedPhoneId {
                            Text("In Use")
                                .foregroundStyle(.secondary)
                        } else {
                            // Switches to this phone; the previous phone is disconnected.
                            Button("Use This Phone") { controller.selectPhone(phone.id) }
                        }
                    } label: {
                        Text(phone.name)
                        if let detail = detail(phone) {
                            Text(detail)
                        }
                    }
                }
                LabeledContent {
                    Button("Pair a New Phone…") { navigator.showPairing() }
                } label: {
                    EmptyView()
                }
            }
            SettingsAccountPcsSection()
        }
        .settingsPaneForm()
        .onAppear {
            controller.reloadPhones()
            accountName = controller.pairing.accountName()
            model.reload()
        }
        .alert("Phone Link Couldn't Sign Out", isPresented: signOutFailurePresented) {
        } message: {
            Text(signOutFailure ?? "")
        }
    }

    /// The phone this Mac uses: the engine's selected phone, else the one the account's list marks.
    private var selectedPhoneId: String? {
        controller.phone?.id ?? controller.phones.first(where: \.selected)?.id
    }

    /// The phone's model (when its name is not already the model) and when the account last saw it.
    private func detail(_ phone: PhoneChoice) -> String? {
        var parts: [String] = []
        if let model = phone.model, model != phone.name { parts.append(model) }
        if phone.lastSeen > 0 {
            let date = Date(timeIntervalSince1970: TimeInterval(phone.lastSeen))
            parts.append(String(localized: "Last seen \(date.formatted(.relative(presentation: .named)))"))
        }
        return parts.isEmpty ? nil : parts.joined(separator: " · ")
    }

    private func signOut() {
        Task {
            do {
                try await controller.signOut()
                accountName = controller.pairing.accountName()
                navigator.showSignIn()
            } catch {
                log.error("sign-out failed: \(error: error)")
                signOutFailure = UserText.describe(error)
            }
        }
    }

    private var signOutFailurePresented: Binding<Bool> {
        Binding(get: { signOutFailure != nil }, set: { if !$0 { signOutFailure = nil } })
    }
}
