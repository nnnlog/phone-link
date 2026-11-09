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
    @State private var confirmingResetRegistration = false
    @State private var signOutFailure: String?
    @State private var resetFailure: String?

    private var isSignedIn: Bool {
        if controller.phase == .signedOut { return false }
        guard let name = accountName, !name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            return false
        }
        return true
    }

    var body: some View {
        Form {
            if !model.isAvailable {
                SettingsUnavailableNote()
            }
            Section("Microsoft Account") {
                if isSignedIn {
                    LabeledContent {
                        Button("Sign Out…") { confirmingSignOut = true }
                            .disabled(!controller.canSignOut)
                    } label: {
                        Text(accountName ?? String(localized: "Microsoft account"))
                        Text("Signed in")
                            .foregroundStyle(.secondary)
                    }
                    .alert("Sign Out of Phone Link?", isPresented: $confirmingSignOut) {
                        Button("Sign Out", role: .destructive) { signOut() }
                        Button("Cancel", role: .cancel) {}
                    } message: {
                        Text("This Mac will stop connecting to your phone, and its copies of your messages, photos, contacts and notifications will be deleted. Your phone stays linked to your Microsoft account.")
                    }
                } else {
                    LabeledContent {
                        Button("Sign In…") { navigator.showSignIn() }
                            .buttonStyle(.borderedProminent)
                    } label: {
                        Text("Not signed in")
                            .font(.headline)
                        Text("Sign in with your Microsoft account to connect and link your phone.")
                            .foregroundStyle(.secondary)
                    }
                }
            }
            Section("Phones") {
                if controller.phones.isEmpty {
                    Text("No phones linked to this account.")
                        .foregroundStyle(.secondary)
                }
                ForEach(controller.phones, id: \.id) { phone in
                    LabeledContent {
                        HStack(spacing: 8) {
                            if phone.id == selectedPhoneId {
                                Text("In Use")
                                    .foregroundStyle(.secondary)
                            } else {
                                // Switches to this phone; the previous phone is disconnected.
                                Button("Use This Phone") { controller.selectPhone(phone.id) }
                            }
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

                // Option to reset pairing / device registration when device is out of sync
                LabeledContent {
                    Button("Reset Device Registration…") { confirmingResetRegistration = true }
                        .disabled(!controller.canSignOut)
                } label: {
                    Text("Having connection or pairing issues?")
                    Text("Forgets this Mac's device registration so you can link your phone afresh.")
                        .foregroundStyle(.secondary)
                }
                .alert("Reset Connection & Device Registration?", isPresented: $confirmingResetRegistration) {
                    Button("Reset Registration", role: .destructive) { resetRegistration() }
                    Button("Cancel", role: .cancel) {}
                } message: {
                    Text("This clears this Mac's registration and pairing credentials. You will need to pair your phone again using a fresh QR code.")
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
        .alert("Phone Link Couldn't Reset Registration", isPresented: resetFailurePresented) {
        } message: {
            Text(resetFailure ?? "")
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

    private func resetRegistration() {
        Task {
            do {
                try await controller.resetDeviceRegistration()
                accountName = controller.pairing.accountName()
                navigator.showPairing()
            } catch {
                log.error("reset registration failed: \(error: error)")
                resetFailure = UserText.describe(error)
            }
        }
    }

    private var signOutFailurePresented: Binding<Bool> {
        Binding(get: { signOutFailure != nil }, set: { if !$0 { signOutFailure = nil } })
    }

    private var resetFailurePresented: Binding<Bool> {
        Binding(get: { resetFailure != nil }, set: { if !$0 { resetFailure = nil } })
    }
}
