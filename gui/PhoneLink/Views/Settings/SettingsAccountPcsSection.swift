import PhoneLinkEngine
import PhoneLinkKit
import SwiftUI

private let log = AppLog(category: "settings")

/// The PCs of the Microsoft account. A Mac gets no device ID from Microsoft that ties its two
/// enrollments together, so the phone finds this Mac's Cross Device enrollment by name and empty ID:
/// an older enrollment of this Mac left on the account can be picked instead, and the phone then shows
/// this Mac as delayed and without its controls. Unlinking those older PCs here removes them.
struct SettingsAccountPcsSection: View {
    @Environment(EngineController.self) private var controller
    @State private var pcs: [AccountPc] = []
    @State private var confirming: AccountPc?
    @State private var unlinking: Set<String> = []
    @State private var failure: String?

    var body: some View {
        Section {
            if pcs.isEmpty {
                Text("No PCs to show.")
                    .foregroundStyle(.secondary)
            }
            ForEach(pcs, id: \.id) { pc in
                LabeledContent {
                    if pc.thisMac {
                        Text("This Mac")
                            .foregroundStyle(.secondary)
                    } else {
                        Button("Unlink…") { confirming = pc }
                            .disabled(unlinking.contains(pc.id))
                    }
                } label: {
                    Text(pc.name.isEmpty ? String(localized: "Unnamed PC") : pc.name)
                    Text(detail(pc))
                }
            }
        } header: {
            Text("PCs")
        } footer: {
            Text("Unlink PCs you no longer use. An older entry for this Mac can keep your phone from reaching it.")
        }
        .onAppear(perform: reload)
        .alert(
            unlinkTitle,
            isPresented: confirmingPresented,
            presenting: confirming
        ) { pc in
            Button("Unlink", role: .destructive) { unlink(pc) }
            Button("Cancel", role: .cancel) {}
        } message: { pc in
            Text("\(detail(pc)). The PC will be removed from your Microsoft account and can't reach your phone until it's linked again.")
        }
        .alert("Phone Link Couldn't Unlink the PC", isPresented: failurePresented) {
        } message: {
            Text(failure ?? "")
        }
    }

    private var unlinkTitle: String {
        guard let pc = confirming, !pc.name.isEmpty else { return String(localized: "Unlink This PC?") }
        return String(localized: "Unlink \(pc.name)?")
    }

    private func detail(_ pc: AccountPc) -> String {
        let app = switch pc.clientType {
        case "PL": String(localized: "Phone Link")
        case "WEA": String(localized: "Cross Device")
        case "": String(localized: "Unknown app")
        default: pc.clientType
        }

        let style = Date.FormatStyle(date: .abbreviated, time: .shortened)
        let linked = pc.registered > 0 ? Date(timeIntervalSince1970: TimeInterval(pc.registered)).formatted(style) : String(localized: "unknown", comment: "A date that is not known")
        let seen = pc.lastSeen > 0 ? Date(timeIntervalSince1970: TimeInterval(pc.lastSeen)).formatted(style) : String(localized: "unknown", comment: "A date that is not known")

        return String(localized: "\(app) · linked \(linked) · last seen \(seen)")
    }

    private func reload() {
        do {
            pcs = try controller.phoneApi.accountPcs()
        } catch FeatureApiError.NotStarted {
            pcs = []
        } catch {
            log.error("account pcs not read: \(error: error)")
            pcs = []
        }
    }

    private func unlink(_ pc: AccountPc) {
        unlinking.insert(pc.id)
        Task {
            do {
                try await controller.phoneApi.unlinkPc(id: pc.id)
            } catch {
                log.error("pc not unlinked: \(error: error)")
                failure = UserText.describe(error)
            }

            unlinking.remove(pc.id)
            reload()
        }
    }

    private var confirmingPresented: Binding<Bool> {
        Binding(get: { confirming != nil }, set: { if !$0 { confirming = nil } })
    }

    private var failurePresented: Binding<Bool> {
        Binding(get: { failure != nil }, set: { if !$0 { failure = nil } })
    }
}
