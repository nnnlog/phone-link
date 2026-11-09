import AppKit
import PhoneLinkEngine
import PhoneLinkKit
import SwiftUI

/// The sign-in window: it hosts Microsoft's sign-in page in a web view while the engine waits, and
/// answers the engine with what the page posts. Before the first sign-in it shows a one-time notice
/// that this Mac is registered with the account as a Windows device. The app closes the window once
/// the engine starts.
struct SignInView: View {
    @Environment(EngineController.self) private var controller
    @AppStorage("didSeeDeviceRegistrationNotice") private var didSeeNotice = false

    var body: some View {
        ZStack {
            // A solid glass background, so the page never shows as floating text over the desktop.
            LinearGradient(
                colors: [
                    Color(red: 0.13, green: 0.12, blue: 0.11),
                    Color(red: 0.07, green: 0.06, blue: 0.055)
                ],
                startPoint: .top,
                endPoint: .bottom
            )
            .ignoresSafeArea()

            VStack(spacing: 14) {
                content
            }
            .frame(width: 520, height: 600)
        }
    }

    @ViewBuilder private var content: some View {
        switch controller.phase {
        case let .webSignIn(signIn):
            if signIn.request.kind == .login && !didSeeNotice {
                notice
            } else {
                webView(signIn)
            }
        case .selectingPhone:
            titled("Choose Your Phone")
            PhonePickerView()
        case .signedOut:
            titled("Sign In to Connect Your Phone")
            explanation("You're signed out. Sign in with the Microsoft account your phone uses with Link to Windows.")
            signInButton
        case .accountRemoved:
            titled("Sign In Again")
            explanation("This Mac was removed from your Microsoft account's devices.")
            signInButton
        case let .failed(message):
            titled("Phone Link Couldn't Connect")
            explanation(Text(message))
            Button("Try Again") { controller.start() }.keyboardShortcut(.defaultAction)
        case .idle, .starting:
            titled("Sign In to Connect Your Phone")
            waiting("Connecting…")
        case .running:
            titled("Signed In")
        }
    }

    private var notice: some View {
        VStack(spacing: 14) {
            Image(nsImage: NSApp.applicationIconImage).resizable().frame(width: 64, height: 64).accessibilityHidden(true)
            Text("Connect Your Phone").font(.title2.weight(.semibold))
            explanation(
                "Phone Link registers this Mac with your Microsoft account as a Windows device so your phone can reach it. Microsoft does not offer a way to remove that registration. You can reset it later in Settings › Advanced."
            )
            Spacer(minLength: 0)
            Button("Continue") { didSeeNotice = true }
                .keyboardShortcut(.defaultAction)
                .buttonStyle(.borderedProminent)
        }
        .padding(28)
    }

    private func webView(_ signIn: WebSignIn) -> some View {
        VStack(spacing: 0) {
            WebSignInWebView(
                request: signIn.request,
                onResult: { json in controller.completeWebSignIn(json: json) }
            )
            // One web view per request: a consent page that follows the sign-in page must not reuse the
            // finished view and its coordinator.
            .id(signIn.requestId)
            HStack {
                Button("Cancel") { controller.cancelWebSignIn(declined: true) }
                    .keyboardShortcut(.cancelAction)
                Spacer()
            }
            .padding(10)
        }
    }

    private var signInButton: some View {
        Button("Sign In") { controller.start() }.keyboardShortcut(.defaultAction)
    }

    private func titled(_ text: LocalizedStringKey) -> some View {
        Text(text).font(.title2.weight(.semibold)).multilineTextAlignment(.center).padding(.top, 24)
    }

    private func explanation(_ text: LocalizedStringKey) -> some View {
        explanation(Text(text))
    }

    private func explanation(_ text: Text) -> some View {
        text
            .foregroundStyle(.secondary)
            .multilineTextAlignment(.center)
            .fixedSize(horizontal: false, vertical: true)
            .padding(.horizontal, 28)
    }

    private func waiting(_ text: LocalizedStringKey) -> some View {
        HStack(spacing: 6) {
            ProgressView().controlSize(.small)
            Text(text).foregroundStyle(.secondary)
        }
        .font(.callout)
    }
}
