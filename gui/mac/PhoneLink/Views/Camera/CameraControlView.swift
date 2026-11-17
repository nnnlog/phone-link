import CoreImage.CIFilterBuiltins
import PhoneLinkEngine
import PhoneLinkKit
import SwiftUI

/// The camera control window: shown while an app uses the phone's camera. It shows the connection,
/// the controls while the video runs (pause, switch cameras, effects), the phone's battery and a
/// poor-network warning, and what to do when something stops the video.
struct CameraControlView: View {
    @Environment(ConnectedCameraModel.self) private var model
    @Environment(EngineController.self) private var controller

    var body: some View {
        VStack(spacing: 16) {
            if let status = model.status {
                content(status)
            } else {
                Text("Phone Link isn't connected.")
                    .foregroundStyle(.secondary)
            }
        }
        .padding(16)
        // Wide enough for Pause, Switch Camera and the effect picker on one line.
        .frame(width: 430)
        .alert(model.failure?.title ?? "", isPresented: failurePresented, presenting: model.failure) { _ in
        } message: { failure in
            Text(failure.message)
        }
    }

    @ViewBuilder private func content(_ status: PhoneCameraStatus) -> some View {
        switch status.state {
        case .connecting:
            ProgressView()
            Text("Connecting to your phone…")
        case .streaming, .paused:
            controls(status)
        case .startStreamNotification:
            takeover(
                symbol: "hand.tap",
                title: String(localized: "Tap the notification on your phone"),
                message: String(localized: "Your phone shows a notification from Link to Windows. Tap it to start the camera.")
            )
        case .error, .terminalError:
            errorView(status)
        case .registered, .unregistered:
            Text("The camera isn't in use.")
                .foregroundStyle(.secondary)
        }
    }

    /// The video runs (or is paused): the camera in use and the phone's battery on the first line,
    /// then Pause or Resume, Switch Camera and the effect picker on one line.
    private func controls(_ status: PhoneCameraStatus) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 8) {
                Circle()
                    .fill(.green)
                    .frame(width: 8, height: 8)
                    .accessibilityHidden(true)
                Text("\(phoneName) Camera")
                    .font(.headline)
                    .lineLimit(1)
                Spacer(minLength: 8)
                if status.batteryLevel >= 0 {
                    HStack(spacing: 3) {
                        Text("\(status.batteryLevel)%")
                            .monospacedDigit()
                        Image(systemName: status.isCharging ? "battery.100.bolt" : "battery.50")
                    }
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                    .accessibilityElement(children: .combine)
                    .accessibilityLabel("Battery \(status.batteryLevel)%")
                }
            }
            if status.lowNetwork {
                Label("Poor connection", systemImage: "wifi.exclamationmark")
                    .font(.callout)
                    .foregroundStyle(.orange)
            }
            HStack(spacing: 8) {
                if status.state == .paused {
                    Button { model.resume() } label: { Label("Resume", systemImage: "play.fill") }
                } else {
                    Button { model.pause() } label: { Label("Pause", systemImage: "pause.fill") }
                }
                Button { model.switchCamera() } label: {
                    Label("Switch Camera", systemImage: "arrow.triangle.2.circlepath.camera")
                }
                .disabled(status.face == nil)
                Spacer(minLength: 8)
                if status.effects.count > 1 {
                    Picker("Effect", selection: effectSelection(status)) {
                        ForEach(Array(status.effects.enumerated()), id: \.offset) { index, effect in
                            Text(ConnectedCameraModel.effectLabel(effect)).tag(index)
                        }
                    }
                    .labelsHidden()
                    .fixedSize()
                }
            }
            if status.state == .paused, status.error != .none {
                Text(ConnectedCameraModel.errorText(status.error).message)
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var phoneName: String { controller.phone?.name ?? String(localized: "Phone") }

    private func effectSelection(_ status: PhoneCameraStatus) -> Binding<Int> {
        Binding(
            get: { Int(status.selectedEffect ?? 0) },
            set: { model.selectEffect($0) }
        )
    }

    @ViewBuilder private func errorView(_ status: PhoneCameraStatus) -> some View {
        let text = ConnectedCameraModel.errorText(status.error)
        takeover(symbol: "exclamationmark.triangle", title: text.title, message: text.message)
        if status.error == .postNotificationPermissionNeeded, let uri = status.qrCodeUri, let code = qrCode(uri) {
            Image(nsImage: code)
                .interpolation(.none)
                .resizable()
                .frame(width: 160, height: 160)
        }
        switch status.errorAction {
        case .refresh: Button("Reconnect") { model.errorAction() }
        case .requestPermissions: Button("Send Request to Phone") { model.errorAction() }
        case .close: Button("Close") { model.errorAction() }
        case .none: EmptyView()
        }
    }

    private func takeover(symbol: String, title: String, message: String) -> some View {
        VStack(spacing: 8) {
            Image(systemName: symbol)
                .font(.system(size: 36))
                .foregroundStyle(.secondary)
            Text(title).font(.headline)
            Text(message)
                .multilineTextAlignment(.center)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private func qrCode(_ text: String) -> NSImage? {
        let filter = CIFilter.qrCodeGenerator()
        filter.message = Data(text.utf8)
        guard let output = filter.outputImage?.transformed(by: CGAffineTransform(scaleX: 8, y: 8)) else { return nil }

        let rep = NSCIImageRep(ciImage: output)
        let image = NSImage(size: rep.size)
        image.addRepresentation(rep)
        return image
    }

    private var failurePresented: Binding<Bool> {
        Binding(get: { model.failure != nil }, set: { if !$0 { model.failure = nil } })
    }
}
