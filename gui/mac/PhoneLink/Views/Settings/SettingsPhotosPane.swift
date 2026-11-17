import PhoneLinkKit
import SwiftUI

/// Photos: whether the phone's recent photos show on this Mac, what may be done with them, and the
/// notifications the phone sends when a photo is taken.
struct SettingsPhotosPane: View {
    @Environment(SettingsModel.self) private var model
    @Environment(RemoteSnippingModel.self) private var newPhotos

    var body: some View {
        @Bindable var model = model
        @Bindable var newPhotos = newPhotos
        Form {
            if !model.isAvailable {
                SettingsUnavailableNote()
            }
            Section {
                Toggle("Show photos on this Mac", isOn: $model.showsPhotos)
                Group {
                    // A phone that sends its own new-photo notifications has the toggle below instead.
                    if newPhotos.photosNotificationsApply {
                        Toggle("Notify me about new photos", isOn: $model.notifiesPhotos)
                    }
                    Toggle("Allow deleting photos from this Mac", isOn: $model.allowsPhotoDeletion)
                }
                .disabled(!model.showsPhotos)
            }
            .disabled(!model.isAvailable)
            if newPhotos.showsToggle {
                Section {
                    Toggle(isOn: $newPhotos.getsNotifications) {
                        Text("Notify me when I take a photo on my phone")
                        Text("Click the notification to open or edit the photo on this Mac")
                    }
                    if newPhotos.needsPermission {
                        SettingsNewPhotoPermissionRow(model: newPhotos)
                    }
                }
            }
        }
        .settingsPaneForm()
        .onAppear {
            model.reload()
            newPhotos.reload()
        }
        .alert(newPhotos.failure?.title ?? "", isPresented: failurePresented, presenting: newPhotos.failure) { _ in
        } message: { failure in
            Text(failure.message)
        }
    }

    private var failurePresented: Binding<Bool> {
        Binding(get: { newPhotos.failure != nil }, set: { if !$0 { newPhotos.failure = nil } })
    }
}

/// The phone has not allowed photo access for new-photo notifications: a link that asks it.
private struct SettingsNewPhotoPermissionRow: View {
    let model: RemoteSnippingModel

    var body: some View {
        LabeledContent("Your phone needs to allow access to photos") {
            if model.requestingPermission {
                HStack(spacing: 6) {
                    ProgressView()
                        .controlSize(.small)
                    Text("Waiting for your phone…")
                        .foregroundStyle(.secondary)
                }
            } else {
                Button("Send Request to Phone") {
                    Task { await model.requestPermission() }
                }
                .buttonStyle(.link)
            }
        }
    }
}
