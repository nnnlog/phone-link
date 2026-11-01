import PhoneLinkEngine
import PhoneLinkKit
import SwiftUI

/// Camera: the phone as a camera on this Mac — the camera extension, the switch, the phone's camera
/// permission, and a failure to publish the camera with its retry.
struct SettingsCameraPane: View {
    @Environment(ConnectedCameraModel.self) private var model

    var body: some View {
        @Bindable var model = model
        Form {
            Section {
                LabeledContent("Camera extension") {
                    extensionState
                }
            } footer: {
                Text("Phone Link adds your phone to this Mac's cameras through a camera extension that macOS asks you to allow once.")
                    .foregroundStyle(.secondary)
            }
            if model.status == nil {
                SettingsUnavailableNote()
            } else if model.showsSetting, let status = model.status {
                Section {
                    Toggle(isOn: $model.cameraEnabled) {
                        Text("Use your phone as a camera")
                        Text("Choose it as the camera in any app on this Mac")
                    }
                    .disabled(!status.macAvailable)
                    if status.cameraEnabled {
                        SettingsAccessRow(
                            title: "Camera access on the phone",
                            allowed: status.cameraPermission,
                            pending: model.requestingPermission
                        ) {
                            Task { await model.requestPermission() }
                        }
                    }
                    if status.registrationFailed {
                        HStack {
                            Label("Your phone couldn't be added as a camera.", systemImage: "exclamationmark.triangle")
                            Spacer()
                            Button("Try Again") { model.retryRegistration() }
                        }
                    }
                    if let replaced = model.replacedPhone {
                        Text("\(replaced) is no longer used as a camera. Only one phone can be a camera at a time.")
                            .foregroundStyle(.secondary)
                    }
                }
            } else {
                Section {
                    Text("This phone can't be used as a camera.")
                        .foregroundStyle(.secondary)
                }
            }
        }
        .settingsPaneForm()
        .onAppear { model.reload() }
        .alert(model.failure?.title ?? "", isPresented: failurePresented, presenting: model.failure) { _ in
        } message: { failure in
            Text(failure.message)
        }
    }

    @ViewBuilder private var extensionState: some View {
        switch model.extensionState {
        case .active:
            Text("On")
        case .notRequested, .activating:
            ProgressView().controlSize(.small)
        case .needsApproval:
            Button("Allow in System Settings…") { model.openExtensionSettings() }
        case .needsRestart:
            Text("Restart this Mac to finish")
        case let .failed(reason):
            HStack {
                Text("Not installed").help(reason)
                Button("Try Again") { model.activateExtension() }
            }
        }
    }

    private var failurePresented: Binding<Bool> {
        Binding(get: { model.failure != nil }, set: { if !$0 { model.failure = nil } })
    }
}
