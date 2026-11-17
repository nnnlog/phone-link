import PhoneLinkEngine
import PhoneLinkKit
import SwiftUI

/// The call window's content: who the call placed or answered on this Mac is with, its state and how
/// long it has run, with *End*, and the other call on hold below it. The call's audio is on the phone,
/// so there is no mute, hold, keypad or audio transfer.
struct CallWindowView: View {
    @Environment(CallControlModel.self) private var model

    var body: some View {
        VStack(spacing: 16) {
            if let call = model.primaryCall {
                PrimaryCallView(call: call)
            }
            if let held = model.secondaryCall {
                Divider()
                SecondaryCallRow(call: held)
            }
            Divider()
            CallsStayOnPhoneNote(text: "Talk on your phone. This Mac only controls the call.")
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(20)
        .frame(width: 320)
        .fixedSize(horizontal: false, vertical: true)
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

private struct PrimaryCallView: View {
    let call: ActiveCall
    @Environment(CallControlModel.self) private var model

    var body: some View {
        VStack(spacing: 8) {
            ActiveCallAvatar(call: call, size: 72)
            Text(call.shownName)
                .font(.title3.weight(.semibold))
                .multilineTextAlignment(.center)
                .lineLimit(2)
            if let number = call.shownNumber {
                Text(number)
                    .foregroundStyle(.secondary)
            }
            CallStatusLine(call: call)
            Button {
                Task { await model.end(call) }
            } label: {
                Label("End", systemImage: "phone.down.fill")
            }
            .buttonStyle(CallActionButtonStyle(fill: .red, width: 120))
            .keyboardShortcut("h", modifiers: [.command, .shift])
            .help("End call")
            .padding(.top, 6)
        }
        .frame(maxWidth: .infinity)
    }
}

/// "Calling…", "On hold", or "On phone" over the call's duration.
private struct CallStatusLine: View {
    let call: ActiveCall

    var body: some View {
        VStack(spacing: 2) {
            Text(status)
                .foregroundStyle(.secondary)
            if call.status == .talking, let start = call.talkingSinceMs {
                TimelineView(.periodic(from: .now, by: 1)) { context in
                    Text(callDuration(since: start, now: context.date))
                        .monospacedDigit()
                        .foregroundStyle(.secondary)
                }
            }
        }
    }

    private var status: String {
        switch call.status {
        case .dialing: String(localized: "Calling…")
        case .held: String(localized: "On hold")
        case .incoming: String(localized: "Incoming Call")
        case .talking: String(localized: "On phone")
        }
    }
}

/// The other call: on hold; a click swaps the two.
private struct SecondaryCallRow: View {
    let call: ActiveCall
    @Environment(CallControlModel.self) private var model

    var body: some View {
        Button {
            Task { await model.swap() }
        } label: {
            HStack(spacing: 10) {
                ActiveCallAvatar(call: call, size: 32)
                VStack(alignment: .leading, spacing: 1) {
                    Text(call.shownName)
                        .lineLimit(1)
                    Text(call.status == .held ? "On hold. Click to switch to this call." : "On phone")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                Spacer()
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(call.status != .held)
    }
}
