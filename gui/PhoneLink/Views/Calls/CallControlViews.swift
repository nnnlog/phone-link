import PhoneLinkEngine
import PhoneLinkKit
import SwiftUI

/// The line every call-control surface carries: the call itself stays on the phone.
struct CallsStayOnPhoneNote: View {
    var text: LocalizedStringKey = "Calls stay on your phone. This Mac places, answers and ends them over Bluetooth, and you talk on your phone or on a headset connected to it."

    var body: some View {
        Label {
            Text(text)
        } icon: {
            Image(systemName: "iphone.gen3")
        }
        .font(.callout)
        .foregroundStyle(.secondary)
    }
}

/// A call button in a solid color with white text: red to decline or end, green to answer or call. It
/// keeps its color in a panel that is not the key window, where a prominent bordered button turns gray.
struct CallActionButtonStyle: ButtonStyle {
    let fill: Color
    /// Fills the width it is offered; otherwise as wide as `width`.
    var width: CGFloat? = nil
    @Environment(\.isEnabled) private var isEnabled

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.body.weight(.semibold))
            .foregroundStyle(.white)
            .lineLimit(1)
            .frame(maxWidth: width ?? .infinity, minHeight: 32)
            .frame(width: width)
            .padding(.horizontal, 10)
            .background(fill.opacity(configuration.isPressed ? 0.7 : 1), in: Capsule())
            .opacity(isEnabled ? 1 : 0.45)
            .contentShape(Capsule())
    }
}

/// The explanation shown the first time call control is switched on on this Mac: the user talks on
/// the phone, not through the Mac. *Turn On* switches it on.
struct CallControlIntroSheet: View {
    @Environment(CallControlModel.self) private var model
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(spacing: 16) {
            Image(systemName: "phone.connection")
                .font(.system(size: 40))
                .foregroundStyle(.tint)
            Text("Calls Stay on Your Phone")
                .font(.title2.weight(.semibold))
            VStack(alignment: .leading, spacing: 10) {
                Text("This Mac can place, answer, decline, and end your phone's calls over Bluetooth.")
                Text("You talk on your phone or on a headset connected to it. This Mac's microphone and speakers aren't used for calls.")
                    .fontWeight(.medium)
            }
            .fixedSize(horizontal: false, vertical: true)
            HStack {
                Button("Cancel", role: .cancel) { dismiss() }
                    .keyboardShortcut(.cancelAction)
                Spacer()
                Button("Turn On") {
                    model.acknowledgeIntro()
                    model.setEnabled(true)
                    dismiss()
                }
                .keyboardShortcut(.defaultAction)
            }
        }
        .padding(24)
        .frame(width: 480)
    }
}

extension View {
    /// Presents the first switch-on's explanation while `isPresented`.
    func callControlIntro(isPresented: Binding<Bool>) -> some View {
        sheet(isPresented: isPresented) {
            CallControlIntroSheet()
        }
    }
}

/// A call-control notice in a strip: its title and text, and its action.
struct CallControlNoticeView: View {
    let notice: CallControlNotice
    @Environment(CallControlModel.self) private var model

    var body: some View {
        // The action beside the text where it fits on one line, else under it.
        ViewThatFits(in: .horizontal) {
            HStack(alignment: .top, spacing: 10) {
                icon
                text
                Spacer(minLength: 8)
                action
            }
            VStack(alignment: .leading, spacing: 8) {
                HStack(alignment: .top, spacing: 10) {
                    icon
                    text
                }
                action
                    .padding(.leading, 30)
            }
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.quinary, in: RoundedRectangle(cornerRadius: 8))
    }

    private var icon: some View {
        Image(systemName: notice.action == .dismiss ? "phone.down" : "antenna.radiowaves.left.and.right.slash")
            .foregroundStyle(.secondary)
            .frame(width: 20)
    }

    private var text: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(notice.title)
                .fontWeight(.semibold)
            Text(notice.message)
                .font(.callout)
                .foregroundStyle(.secondary)
        }
    }

    @ViewBuilder
    private var action: some View {
        if model.isConnecting && notice.action != .dismiss && notice.action != .openBluetoothSettings {
            ProgressView()
                .controlSize(.small)
        } else {
            Button(notice.actionTitle) {
                Task { await model.act(on: notice) }
            }
            .controlSize(.small)
            .fixedSize()
        }
    }
}

/// *Call* for a number: places the call when call control is ready; while call control is off,
/// switches it on (explaining first, the first time).
struct CallButton: View {
    let number: String
    var iconOnly = false
    @Environment(CallControlModel.self) private var model
    @State private var introShown = false

    var body: some View {
        Button {
            if model.isEnabled && model.hasCallHistory {
                Task { await model.dial(number) }
            } else if model.needsIntro {
                introShown = true
            } else {
                model.setEnabled(true)
            }
        } label: {
            if iconOnly {
                Label("Call", systemImage: "phone").labelStyle(.iconOnly)
            } else {
                Label("Call", systemImage: "phone")
            }
        }
        .disabled(!enabled)
        .help(help)
        .callControlIntro(isPresented: $introShown)
    }

    private var enabled: Bool {
        guard !number.isEmpty, model.isOffered, model.hasCallHistory else { return false }
        return !model.isEnabled || model.canDial
    }

    private var help: String {
        if number.isEmpty { return String(localized: "No number to call") }
        if !model.hasCallHistory { return String(localized: "To call from this Mac, turn on call history in Settings › Calls") }
        if !model.isEnabled { return String(localized: "Turn on calls on this Mac") }
        if !model.canDial { return String(localized: "This Mac isn't connected to your phone") }
        return String(localized: "Call \(number) using your phone")
    }
}

/// A current call's picture: its contact's photo, else the contact's monogram, else a person symbol.
struct ActiveCallAvatar: View {
    let call: ActiveCall
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

extension ActiveCall {
    /// The contact's name, else the number, else "Unknown".
    var shownName: String {
        if let name, !name.isEmpty { return name }
        return number.isEmpty ? String(localized: "Unknown") : number
    }

    /// The number under a contact's name; `nil` when the name is the number.
    var shownNumber: String? {
        (name?.isEmpty == false && !number.isEmpty) ? number : nil
    }
}

/// `m:ss` (or `h:mm:ss`) since `start`.
func callDuration(since startMs: Int64, now: Date) -> String {
    let seconds = max(0, Int(now.timeIntervalSince1970 - Double(startMs) / 1000))
    let (h, m, s) = (seconds / 3600, seconds / 60 % 60, seconds % 60)
    return h > 0 ? String(format: "%d:%02d:%02d", h, m, s) : String(format: "%d:%02d", m, s)
}
