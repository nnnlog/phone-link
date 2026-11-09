import PhoneLinkEngine
import PhoneLinkKit
import SwiftUI

/// Pairing a phone to this Microsoft account: the QR code to scan with the phone, the PIN to confirm
/// or type on it, the manual instructions, and what the session waits for. It starts a QR session when
/// it appears with none. `onCancel`, when given, offers a way out (the phone list, or closing the
/// window).
struct PairingView: View {
    @Environment(PairingModel.self) private var model
    @Environment(\.colorScheme) private var colorScheme
    var onCancel: (() -> Void)?

    var body: some View {
        ZStack {
            // Unified Apple Glass background filling the entire modal window
            VisualEffectView(material: .underWindowBackground, blendingMode: .behindWindow, state: .active)
                .ignoresSafeArea()

            LinearGradient(
                colors: colorScheme == .dark ? [
                    Color.black.opacity(0.35),
                    Color.black.opacity(0.20)
                ] : [
                    Color.white.opacity(0.18),
                    Color.white.opacity(0.08)
                ],
                startPoint: .topLeading,
                endPoint: .bottomTrailing
            )
            .ignoresSafeArea()

            VStack(spacing: 14) {
                // Window traffic lights clearance
                HStack {
                    Spacer()
                }
                .frame(height: 16)

                // Header icon & title
                HStack(spacing: 8) {
                    Image(systemName: "iphone.badge.play")
                        .font(.system(size: 18, weight: .semibold))
                        .foregroundStyle(Color.accentColor)

                    Text("Pair Your Phone")
                        .font(.system(size: 16, weight: .bold))
                        .foregroundStyle(Color.primary)
                }

                // Main page content: seamlessly part of the modal body
                VStack(spacing: 14) {
                    page
                }
                .frame(maxWidth: .infinity)
                .padding(.horizontal, 20)
                .padding(.vertical, 8)

                if let onCancel, model.page != .paired {
                    Button("Cancel", action: onCancel)
                        .keyboardShortcut(.cancelAction)
                        .buttonStyle(.plain)
                        .font(.system(size: 12.5, weight: .medium))
                        .foregroundStyle(Color.secondary)
                        .padding(.top, 2)
                }

                Spacer(minLength: 4)
            }
            .multilineTextAlignment(.center)
            .padding(16)
        }
        .onAppear {
            if model.page == .idle { model.start(.qrCode) }
        }
    }

    @ViewBuilder private var page: some View {
        switch model.page {
        case .idle:
            Button("Show QR Code") { model.start(.qrCode) }
                .buttonStyle(.borderedProminent)
        case .loading:
            progress("Getting a code…")
        case let .qrCode(code, _):
            title("Scan the QR Code with Your Phone")
            explanation("On your Android phone, open the camera or Link to Windows and scan this code. Then follow the steps on the phone.")
            PairingQRCodeView(text: code.value)
            ExpiryText(code: code)
            Button("Pair Manually") { model.showManualInstructions() }
                .buttonStyle(.link)
        case let .manualInstructions(link, accountName):
            title("Pair Manually")
            VStack(alignment: .leading, spacing: 10) {
                step(1, "On your phone, open this link:")
                Text(link)
                    .font(.callout.monospaced())
                    .textSelection(.enabled)
                    .padding(.leading, 22)
                if let accountName {
                    step(2, "Sign in with \(accountName).")
                } else {
                    step(2, "Sign in with the Microsoft account you use on this Mac.")
                }
                step(3, "Continue here to get a PIN, then enter it in Link to Windows.")
            }
            .multilineTextAlignment(.leading)
            .frame(maxWidth: 380, alignment: .leading)
            HStack {
                Button("Use QR Code") { model.backToQrCode() }
                Button("Continue") { model.continueManually() }
                    .keyboardShortcut(.defaultAction)
                    .buttonStyle(.borderedProminent)
            }
        case let .pin(code, kind):
            title(kind == .challenge ? "Enter This PIN on Your Phone" : "Enter This PIN in Link to Windows")
            PinText(pin: code.value, struck: false)
            ExpiryText(code: code)
        case let .pinProblem(code, kind, problem, refreshing, failure):
            title(problem == .expired ? "This PIN Expired" : "Your Phone Entered a Different PIN")
            PinText(pin: code.value, struck: true)
            if let failure {
                explanation(Text(failure.message))
            }
            if kind == .challenge {
                if refreshing {
                    progress("Getting a new PIN…")
                } else {
                    Button("Get a New PIN") { model.refreshPin() }
                        .keyboardShortcut(.defaultAction)
                        .buttonStyle(.borderedProminent)
                }
            } else {
                Button("Start Over") { model.start(.manualPin) }
                    .keyboardShortcut(.defaultAction)
                    .buttonStyle(.borderedProminent)
            }
        case .waitingForPermissions:
            title("Finish on Your Phone")
            explanation("Allow the permissions Link to Windows asks for on your phone.")
            progress("Waiting for your phone…")
        case .completing:
            progress("Finishing up…")
        case .paired:
            Image(systemName: "checkmark.circle.fill")
                .font(.system(size: 44))
                .foregroundStyle(.green)
                .accessibilityHidden(true)
            title("Your Phone Is Paired")
            explanation("Your phone is now linked. You can close this window to start using Phone Link.")
            if let onCancel {
                Button("Done", action: onCancel)
                    .keyboardShortcut(.defaultAction)
                    .buttonStyle(.borderedProminent)
                    .padding(.top, 6)
            }
        case let .failed(failure):
            Image(systemName: "exclamationmark.triangle")
                .font(.system(size: 36))
                .foregroundStyle(.secondary)
                .accessibilityHidden(true)
            explanation(Text(failure.message))
            Button(failure == .expired ? "Get a New Code" : "Try Again") { model.start(model.flow) }
                .keyboardShortcut(.defaultAction)
                .buttonStyle(.borderedProminent)
        }
    }

    private func title(_ text: LocalizedStringKey) -> some View {
        Text(text)
            .font(.system(size: 15, weight: .bold))
            .foregroundStyle(Color.primary)
    }

    private func explanation(_ text: LocalizedStringKey) -> some View {
        explanation(Text(text))
    }

    private func explanation(_ text: Text) -> some View {
        text
            .font(.system(size: 12.5))
            .foregroundStyle(Color.secondary)
            .fixedSize(horizontal: false, vertical: true)
            .frame(maxWidth: 380)
    }

    private func progress(_ text: LocalizedStringKey) -> some View {
        HStack(spacing: 8) {
            ProgressView().controlSize(.small)
            Text(text)
                .font(.system(size: 13, weight: .medium))
                .foregroundStyle(Color.secondary)
        }
        .padding(.vertical, 12)
    }

    private func step(_ number: Int, _ text: LocalizedStringKey) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Text("\(number).").monospacedDigit().foregroundStyle(.secondary)
            Text(text).fixedSize(horizontal: false, vertical: true)
        }
    }
}

/// The QR code, drawn module by module (no smoothing) on white in both appearances, as scanners need.
private struct PairingQRCodeView: View {
    let text: String
    @Environment(\.colorScheme) private var colorScheme

    var body: some View {
        if let image = PairingQRCode.image(for: text) {
            ZStack {
                RoundedRectangle(cornerRadius: 16, style: .continuous)
                    .fill(Color.white)
                    .shadow(color: Color.black.opacity(colorScheme == .dark ? 0.35 : 0.08), radius: 8, x: 0, y: 3)

                Image(decorative: image, scale: 1)
                    .interpolation(.none)
                    .resizable()
                    .frame(width: 200, height: 200)
                    .accessibilityLabel("QR code for pairing your phone")
            }
            .frame(width: 224, height: 224)
            .padding(.vertical, 4)
        } else {
            Text("The code can't be shown.").foregroundStyle(.secondary)
        }
    }
}

/// A PIN in large spaced digits; struck through once it no longer works.
private struct PinText: View {
    let pin: String
    let struck: Bool

    var body: some View {
        Text(pin)
            .font(.system(size: 40, weight: .semibold, design: .monospaced))
            .tracking(6)
            .strikethrough(struck)
            .foregroundStyle(struck ? .secondary : .primary)
            .textSelection(.enabled)
            .accessibilityLabel("PIN \(pin.map(String.init).joined(separator: " "))")
    }
}

/// "Expires in N minutes", counting down to the code's expiry on this Mac's clock.
private struct ExpiryText: View {
    let code: PairingCode

    var body: some View {
        TimelineView(.periodic(from: .now, by: 15)) { context in
            let expiry = Date(timeIntervalSince1970: TimeInterval(code.expiresAt))
            let minutes = Int((expiry.timeIntervalSince(context.date) / 60).rounded())
            Text(minutes >= 1 ? "Expires in \(minutes) min" : "Expires in less than a minute")
                .font(.caption)
                .foregroundStyle(.secondary)
        }
    }
}
