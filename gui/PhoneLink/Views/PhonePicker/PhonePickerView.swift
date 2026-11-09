import PhoneLinkEngine
import PhoneLinkKit
import SwiftUI

/// The account's linked phones, to choose the one Phone Link connects to. Shown after sign-in while no
/// phone is selected; the other phones stay linked and are not connected. With no linked phone it pairs
/// one by QR code at once; with phones, *Pair a New Phone* pairs another.
struct PhonePickerView: View {
    @Environment(EngineController.self) private var controller
    @Environment(PairingModel.self) private var pairing
    @State private var choice: String?
    @State private var pairingShown = false

    var body: some View {
        VStack(spacing: 12) {
            if controller.phones.isEmpty {
                PairingView()
                Divider().frame(maxWidth: 320)
                Text("Already linked your phone in Link to Windows?")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
                    .fixedSize(horizontal: false, vertical: true)
                Button("Check Again") { controller.reloadPhones() }
            } else if pairingShown {
                PairingView(onCancel: {
                    pairing.cancel()
                    pairingShown = false
                })
            } else {
                Text("Choose the phone to use with this Mac. You can change it later in Settings.")
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
                    .fixedSize(horizontal: false, vertical: true)
                List(controller.phones, id: \.id, selection: $choice) { phone in
                    PhoneChoiceRow(phone: phone)
                }
                .frame(minHeight: 80, maxHeight: 160)
                HStack {
                    Button("Pair a New Phone…") {
                        pairing.cancel()
                        pairingShown = true
                    }
                    Button("Connect") {
                        if let choice { controller.selectPhone(choice) }
                    }
                    .keyboardShortcut(.defaultAction)
                    .disabled(choice == nil)
                }
            }
        }
        .onAppear {
            if choice == nil, controller.phones.count == 1 { choice = controller.phones.first?.id }
        }
    }
}

/// One linked phone: its name and model, and when the account last saw it.
struct PhoneChoiceRow: View {
    let phone: PhoneChoice

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(phone.name)
            if let detail {
                Text(detail)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
    }

    private var detail: String? {
        var parts: [String] = []
        if let model = phone.model, model != phone.name { parts.append(model) }
        if phone.lastSeen > 0 {
            let date = Date(timeIntervalSince1970: TimeInterval(phone.lastSeen))
            parts.append(String(localized: "Last seen \(date.formatted(.relative(presentation: .named)))"))
        }
        return parts.isEmpty ? nil : parts.joined(separator: " · ")
    }
}
