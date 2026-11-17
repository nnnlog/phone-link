import PhoneLinkEngine
import PhoneLinkKit
import SwiftUI

/// The Photos section's column: the phone's recent photos as a grid, or the one state that keeps
/// them from showing, each with the single action that resolves it.
struct PhotoGridColumn: View {
    @Environment(PhotosModel.self) private var model
    @Environment(EngineController.self) private var controller

    var body: some View {
        content
            .navigationTitle("Photos")
            .alert(
                model.prompt?.title ?? "",
                isPresented: Binding(get: { model.prompt != nil }, set: { if !$0 { model.prompt = nil } }),
                presenting: model.prompt
            ) { prompt in
                switch prompt {
                case let .confirmDelete(ids, _):
                    Button("Delete", role: .destructive) { model.confirmDelete(ids) }
                    Button("Cancel", role: .cancel) {}
                case .allowDeleting:
                    Button("Allow on Phone") { model.allowDeletingOnPhone() }
                    Button("Cancel", role: .cancel) {}
                case .failure:
                    Button("OK") {}
                }
            } message: { prompt in
                Text(prompt.message)
            }
            .onAppear {
                PhotosOpenWithApps.forget()
                model.galleryEntered()
            }
            .onDisappear { model.galleryLeft() }
    }

    @ViewBuilder private var content: some View {
        switch model.content {
        case .nothing:
            Color.clear
        case .unavailable:
            ContentUnavailableView(
                "Photos Aren't Available",
                systemImage: "photo.on.rectangle",
                description: Text("Link to Windows on \(phoneName) doesn't share photos with this Mac.")
            )
        case .turnedOff:
            ContentUnavailableView {
                Label("Photos Are Off on This Mac", systemImage: "photo.on.rectangle")
            } description: {
                Text("Turn on Photos to see recent photos from \(phoneName).")
            } actions: {
                Button("Turn On") { model.turnOn() }
                    .buttonStyle(.borderedProminent)
            }
        case .needsPhoneAccess:
            ContentUnavailableView {
                Label("Allow Photo Access on Your Phone", systemImage: "photo.on.rectangle.angled")
            } description: {
                Text("Recent photos from \(phoneName) appear here after you allow access on the phone.")
            } actions: {
                Button("Send Request to Phone") { model.requestPhotoAccess() }
                    .buttonStyle(.borderedProminent)
                    .disabled(model.requestPending)
            }
        case .empty where model.state?.fullAccessReminder == true:
            // The phone shares only the photos the user picked, and none so far.
            ContentUnavailableView {
                Label("No Photos", systemImage: "photo.on.rectangle")
            } description: {
                Text("Phone Link can see only some of your photos.")
            } actions: {
                Button("Allow Full Access") { model.requestFullAccess() }
                    .buttonStyle(.borderedProminent)
                    .disabled(model.requestPending)
            }
            .navigationSubtitle(subtitle)
        case .empty:
            ContentUnavailableView(
                "No Photos",
                systemImage: "photo.on.rectangle",
                description: Text("Recent photos from \(phoneName) appear here.")
            )
            .navigationSubtitle(subtitle)
        case .gallery:
            PhotosGrid()
                .navigationSubtitle(model.selection.isEmpty ? subtitle : String(localized: "\(model.selection.count) Selected"))
        }
    }

    private var subtitle: String {
        model.state?.syncing == true ? String(localized: "Syncing…") : String(localized: "Recent photos on \(phoneName)")
    }

    private var phoneName: String {
        controller.phone?.name ?? String(localized: "your phone")
    }
}

/// The full-access reminder: the phone shares only the photos the user picked. One line above the
/// grid, gone once dismissed until the next connection.
struct PhotosFullAccessNotice: View {
    @Environment(PhotosModel.self) private var model

    var body: some View {
        HStack(spacing: 8) {
            Text("Phone Link can see only some of your photos.")
                .foregroundStyle(.secondary)
            Spacer(minLength: 8)
            Button("Not Now") { model.dismissFullAccessReminder() }
            Button("Allow Full Access") { model.requestFullAccess() }
                .disabled(model.requestPending)
        }
        .font(.callout)
        .controlSize(.small)
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
    }
}
