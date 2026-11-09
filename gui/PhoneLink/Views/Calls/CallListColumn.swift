import AppKit
import PhoneLinkEngine
import PhoneLinkKit
import SwiftUI

/// The Calls section's content column: the phone's recent calls under their days — or the state that
/// stands in for them: call history off on this Mac, a phone whose Link to Windows needs an update,
/// notifications or call history not allowed on the phone, calls on their way, or no calls at all.
struct CallListColumn: View {
    @Environment(CallsModel.self) private var model
    @Environment(CallControlModel.self) private var control
    @Environment(EngineController.self) private var controller
    @State private var keypadShown = false
    @State private var introShown = false

    var body: some View {
        @Bindable var model = model
        VStack(spacing: 0) {
            // Inline Search & Keypad Bar
            HStack(spacing: 8) {
                HStack(spacing: 6) {
                    Image(systemName: "magnifyingglass")
                        .foregroundStyle(.secondary)
                        .font(.system(size: 13))
                    TextField("Search", text: $model.searchText)
                        .textFieldStyle(.plain)
                        .font(.system(size: 13))
                    if !model.searchText.isEmpty {
                        Button {
                            model.searchText = ""
                        } label: {
                            Image(systemName: "xmark.circle.fill")
                                .foregroundStyle(.secondary)
                                .font(.system(size: 12))
                        }
                        .buttonStyle(.plain)
                    }
                }
                .padding(.horizontal, 8)
                .padding(.vertical, 6)
                .background(Color.primary.opacity(0.06), in: RoundedRectangle(cornerRadius: 8, style: .continuous))

                if control.isOffered && control.hasCallHistory {
                    Button {
                        if control.isEnabled {
                            keypadShown = true
                        } else if control.needsIntro {
                            introShown = true
                        } else {
                            control.setEnabled(true)
                        }
                    } label: {
                        Image(systemName: "circle.grid.3x3")
                            .font(.system(size: 14, weight: .medium))
                            .foregroundStyle(Color.primary)
                            .frame(width: 28, height: 28)
                            .background(Color.primary.opacity(0.08), in: RoundedRectangle(cornerRadius: 7, style: .continuous))
                    }
                    .buttonStyle(.plain)
                    .help(control.isEnabled ? "Call a number using your phone" : "Turn on calls on this Mac")
                    .popover(isPresented: $keypadShown, arrowEdge: .bottom) {
                        DialPadView()
                            .environment(control)
                    }
                }
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 8)

            Divider()

            if let notice = control.dialNotice ?? control.connectionNotice {
                CallControlNoticeView(notice: notice)
                    .padding(.horizontal, 10)
                    .padding(.vertical, 6)
            }
            content
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .callControlIntro(isPresented: $introShown)
        .sheet(item: $model.postNotificationHelp) { help in
            PostNotificationSheet(help: help, phoneName: phoneName)
        }
        .onAppear { model.viewAppeared() }
        .onDisappear { model.viewDisappeared() }
        .navigationSplitViewColumnWidth(min: 260, ideal: 300, max: 420)
    }

    @ViewBuilder
    private var content: some View {
        switch model.content {
        case .nothing:
            Color.clear
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        case .turnedOff:
            ContentUnavailableView {
                Label("Call History Is Off on This Mac", systemImage: "phone")
            } description: {
                Text("Turn on call history to see your phone's recent calls on this Mac.")
            } actions: {
                Button("Turn On") { model.turnOn() }
                    .buttonStyle(.borderedProminent)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        case .updateRequired:
            ContentUnavailableView {
                Label("Update Link to Windows", systemImage: "arrow.down.app")
            } description: {
                Text("To see recent calls, update Link to Windows on \(phoneName), then reconnect.")
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        case .needsNotifications:
            ContentUnavailableView {
                Label("Allow Notifications on Your Phone", systemImage: "phone")
            } description: {
                Text("Allow Link to Windows to send notifications on \(phoneName) to see your recent calls and use other Phone Link features.")
            } actions: {
                Button("Show Me How") { model.showPostNotificationHelp() }
                    .buttonStyle(.borderedProminent)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
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
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        case .updating:
            ProgressView("Updating call history…")
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        case .empty:
            ContentUnavailableView("No Recent Calls", systemImage: "phone", description: Text("Calls from your phone appear here."))
                .frame(maxWidth: .infinity, maxHeight: .infinity)
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

/// What can be done with a call: call its number back on the phone, write to it, show its contact,
/// copy the number.
struct CallActions: View {
    let call: CallHistoryItem
    @Environment(CallsModel.self) private var model
    @Environment(CallControlModel.self) private var control

    var body: some View {
        if control.isEnabled && control.hasCallHistory {
            Button("Call", systemImage: "phone") {
                Task { await control.dial(call.number) }
            }
            .disabled(call.number.isEmpty || !control.canDial)
        }
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
