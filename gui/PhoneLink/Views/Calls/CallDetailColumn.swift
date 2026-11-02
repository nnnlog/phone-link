import PhoneLinkEngine
import PhoneLinkKit
import SwiftUI

/// The Calls section's detail column: the selected call's person, what can be done with their number,
/// and the calls with that number; and the failures of the section's actions.
struct CallDetailColumn: View {
    @Environment(CallsModel.self) private var model

    var body: some View {
        Group {
            if model.content != .list {
                // The content column already says why there is no list.
                Color.clear
            } else if let call = model.selectedCall {
                CallCardView(call: call, calls: model.callsWithSelectedNumber)
                    .id(call.callId)
            } else {
                ContentUnavailableView("No Call Selected", systemImage: "phone")
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .alert(model.alert?.title ?? "", isPresented: alertShown, presenting: model.alert) { _ in
            Button("OK") {}
        } message: { alert in
            Text(alert.message)
        }
    }

    private var alertShown: Binding<Bool> {
        Binding(get: { model.alert != nil }, set: { if !$0 { model.alert = nil } })
    }
}

/// One call's person: their picture, name and number over the actions, then every call with the number.
private struct CallCardView: View {
    let call: CallHistoryItem
    /// The calls with the number, newest first; the selected call alone for a private number.
    let calls: [CallHistoryItem]
    @Environment(CallsModel.self) private var model

    /// The card's widest line, as the contact card's.
    private static let cardWidth: CGFloat = 560

    var body: some View {
        ScrollView {
            VStack(spacing: 20) {
                header
                actions
                VStack(alignment: .leading, spacing: 0) {
                    ForEach(Array((calls.isEmpty ? [call] : calls).enumerated()), id: \.element.callId) { index, entry in
                        CallEntryRow(call: entry, time: model.format.fullTime(ms: entry.startMs), selected: entry.callId == call.callId)
                            .overlay(alignment: .top) {
                                if index > 0 {
                                    Divider()
                                }
                            }
                    }
                }
                .padding(.horizontal, 14)
                .background(.quinary, in: RoundedRectangle(cornerRadius: 10))
            }
            .frame(maxWidth: Self.cardWidth)
            .padding(24)
            .frame(maxWidth: .infinity)
        }
    }

    /// The picture, the name, and the number under a contact's name.
    private var header: some View {
        VStack(spacing: 6) {
            CallAvatar(call: call, size: 88)
                .padding(.bottom, 4)
            Text(CallsFormat.name(call))
                .font(.title2.weight(.semibold))
                .multilineTextAlignment(.center)
                .textSelection(.enabled)
            if call.name?.isEmpty == false && !call.number.isEmpty {
                Text(call.number)
                    .foregroundStyle(.secondary)
                    .textSelection(.enabled)
            }
        }
        .frame(maxWidth: .infinity)
    }

    private var actions: some View {
        HStack(spacing: 12) {
            Button {
                model.message(call)
            } label: {
                Label("Message", systemImage: "message")
            }
            .disabled(call.number.isEmpty)
            .help(call.number.isEmpty ? "A private number can't be messaged" : "Send a message to \(call.number)")
            if call.contactId != nil {
                Button {
                    model.showContact(call)
                } label: {
                    Label("Contact", systemImage: "person.crop.circle")
                }
                .help("Show in Contacts")
            }
        }
        .controlSize(.large)
    }
}

/// One call with the number: what kind, when, and how long it lasted.
private struct CallEntryRow: View {
    let call: CallHistoryItem
    let time: String
    let selected: Bool

    var body: some View {
        let style = CallsFormat.style(call.kind)
        HStack(spacing: 8) {
            Image(systemName: style.symbol ?? "phone")
                .foregroundStyle(style.missed ? AnyShapeStyle(.red) : AnyShapeStyle(.secondary))
                .opacity(style.symbol == nil ? 0 : 1)
                .frame(width: 20)
            VStack(alignment: .leading, spacing: 2) {
                Text(time)
                    .fontWeight(selected ? .semibold : .regular)
                if !style.label.isEmpty {
                    Text(style.label)
                        .font(.caption)
                        .foregroundStyle(style.missed ? AnyShapeStyle(.red) : AnyShapeStyle(.secondary))
                }
            }
            Spacer(minLength: 8)
            if let duration = CallsFormat.duration(seconds: call.durationSecs) {
                Text(duration)
                    .monospacedDigit()
                    .foregroundStyle(.secondary)
            }
        }
        .padding(.vertical, 8)
        .accessibilityElement(children: .combine)
    }
}
