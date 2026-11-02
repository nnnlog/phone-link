import AppKit
import PhoneLinkEngine
import PhoneLinkKit
import SwiftUI

/// The Calls section's content column: the phone's recent calls under their days — or the state that
/// stands in for them: call history off on this Mac, a phone whose Link to Windows needs an update,
/// notifications or call history not allowed on the phone, calls on their way, or no calls at all.
struct CallListColumn: View {
    @Environment(CallsModel.self) private var model
    @Environment(EngineController.self) private var controller

    var body: some View {
        @Bindable var model = model
        content
            .navigationTitle("Calls")
            // The search matches names and the numbers' digits.
            .searchable(text: $model.searchText, prompt: "Search")
            .toolbar {
                if model.isUpdating && model.content == .list {
                    ToolbarItem(placement: .status) {
                        ProgressView()
                            .controlSize(.small)
                            .help("Updating call history…")
                    }
                }
            }
            .sheet(item: $model.postNotificationHelp) { help in
                PostNotificationSheet(help: help, phoneName: phoneName)
            }
            .onAppear { model.viewAppeared() }
            .onDisappear { model.viewDisappeared() }
            // Outermost: under `searchable` and `toolbar` the split view left the column at its minimum.
            .navigationSplitViewColumnWidth(min: 260, ideal: 300, max: 420)
    }

    @ViewBuilder
    private var content: some View {
        switch model.content {
        case .nothing:
            Color.clear
        case .turnedOff:
            ContentUnavailableView {
                Label("Call History Is Off on This Mac", systemImage: "phone")
            } description: {
                Text("Turn on call history to see your phone's recent calls on this Mac.")
            } actions: {
                Button("Turn On") { model.turnOn() }
                    .buttonStyle(.borderedProminent)
            }
        case .updateRequired:
            ContentUnavailableView {
                Label("Update Link to Windows", systemImage: "arrow.down.app")
            } description: {
                Text("To see recent calls, update Link to Windows on \(phoneName), then reconnect.")
            }
        case .needsNotifications:
            ContentUnavailableView {
                Label("Allow Notifications on Your Phone", systemImage: "phone")
            } description: {
                Text("Allow Link to Windows to send notifications on \(phoneName) to see your recent calls and use other Phone Link features.")
            } actions: {
                Button("Show Me How") { model.showPostNotificationHelp() }
                    .buttonStyle(.borderedProminent)
            }
        case .needsPhoneAccess:
            ContentUnavailableView {
                Label("Allow Call History on Your Phone", systemImage: "phone")
            } description: {
                if model.permissionTimedOut {
                    Text("Your phone didn't respond. Unlock it and try again.")
                } else {
                    Text("Recent calls from \(phoneName) appear here after you allow access on the phone.")
                }
            } actions: {
                if model.isRequestingPermission {
                    ProgressView("Check your phone for a permission request…")
                        .controlSize(.small)
                } else {
                    Button("Send Request to Phone") {
                        Task { await model.requestPermission() }
                    }
                    .buttonStyle(.borderedProminent)
                }
            }
        case .updating:
            ProgressView("Updating call history…")
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        case .empty:
            ContentUnavailableView("No Recent Calls", systemImage: "phone", description: Text("Calls from your phone appear here."))
        case .list:
            callList
        }
    }

    private var phoneName: String {
        controller.phone?.name ?? String(localized: "your phone")
    }

    private var callList: some View {
        @Bindable var model = model
        return List(selection: $model.selection) {
            ForEach(model.sections) { section in
                Section(section.title) {
                    ForEach(section.calls, id: \.callId) { call in
                        CallRow(call: call, time: model.format.rowTime(ms: call.startMs))
                            .tag(call.callId)
                            .contextMenu {
                                CallActions(call: call)
                            }
                    }
                }
            }
        }
        .overlay {
            if model.searchFoundNothing {
                ContentUnavailableView.search(text: model.searchText)
            }
        }
    }
}

/// One call of the list: who, what kind of call, and when.
private struct CallRow: View {
    let call: CallHistoryItem
    let time: String
    /// A selected row in a key window is drawn on the accent color: the missed red turns white there.
    @Environment(\.backgroundProminence) private var prominence

    var body: some View {
        let style = CallsFormat.style(call.kind)
        HStack(spacing: 10) {
            CallAvatar(call: call, size: 36)
            VStack(alignment: .leading, spacing: 2) {
                Text(CallsFormat.name(call))
                    .font(.body)
                    .foregroundStyle(nameStyle(missed: style.missed))
                    .lineLimit(1)
                if let symbol = style.symbol {
                    Label(style.label, systemImage: symbol)
                        .font(.subheadline)
                        .foregroundStyle(kindStyle(missed: style.missed))
                        .labelStyle(.titleAndIcon)
                        .lineLimit(1)
                }
            }
            Spacer(minLength: 4)
            Text(time)
                .font(.subheadline)
                .foregroundStyle(.secondary)
                .lineLimit(1)
        }
        .padding(.vertical, 2)
        .accessibilityElement(children: .combine)
    }

    private func nameStyle(missed: Bool) -> AnyShapeStyle {
        missed && prominence != .increased ? AnyShapeStyle(.red) : AnyShapeStyle(.primary)
    }

    private func kindStyle(missed: Bool) -> AnyShapeStyle {
        missed && prominence != .increased ? AnyShapeStyle(.red) : AnyShapeStyle(.secondary)
    }
}

/// A call's picture: its contact's photo when the phone has one, else the contact's monogram, else a
/// person symbol.
struct CallAvatar: View {
    let call: CallHistoryItem
    let size: CGFloat

    var body: some View {
        let name = call.name.flatMap { $0.isEmpty ? nil : $0 }
        if let photoId = call.photoContactId {
            ContactAvatar(contactId: photoId, name: name ?? "", hasPhoto: true, size: size)
        } else {
            Monogram(name: name, size: size)
        }
    }
}

/// What can be done with a call without placing one: write to its number, show its contact, copy the
/// number.
struct CallActions: View {
    let call: CallHistoryItem
    @Environment(CallsModel.self) private var model

    var body: some View {
        Button("Send Message", systemImage: "message") { model.message(call) }
            .disabled(call.number.isEmpty)
        if call.contactId != nil {
            Button("Show Contact", systemImage: "person.crop.circle") { model.showContact(call) }
        }
        Divider()
        Button("Copy Number", systemImage: "doc.on.doc") {
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(call.number, forType: .string)
        }
        .disabled(call.number.isEmpty)
    }
}
