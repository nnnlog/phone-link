import PhoneLinkEngine
import PhoneLinkKit
import SwiftUI

/// The Contacts section's content column: the phone's contacts under their index letters, filtered by
/// the search — or the state that stands in for them: contacts off on this Mac, not allowed on the
/// phone, the first sync, or none at all.
struct ContactListColumn: View {
    @Environment(ContactsModel.self) private var model
    @Environment(EngineController.self) private var controller

    var body: some View {
        @Bindable var model = model
        content
            .navigationTitle("Contacts")
            // The search matches names, the other fields and the numbers' digits.
            .searchable(text: $model.searchText, prompt: "Search")
            .toolbar {
                if model.isSyncing && model.content == .list {
                    ToolbarItem(placement: .status) {
                        ProgressView()
                            .controlSize(.small)
                            .help("Syncing contacts…")
                    }
                }
            }
            // Outermost: under `searchable` and `toolbar` the split view left the column at its minimum.
            .navigationSplitViewColumnWidth(min: 260, ideal: 300, max: 420)
    }

    @ViewBuilder
    private var content: some View {
        switch model.content {
        case .nothing:
            Color.clear
        case .turnedOff:
            ContentUnavailableView {
                Label("Contacts Are Off on This Mac", systemImage: "person.crop.circle")
            } description: {
                Text("Turn on contacts to see your phone's contacts on this Mac.")
            } actions: {
                Button("Turn On") { model.turnOn() }
                    .buttonStyle(.borderedProminent)
            }
        case let .needsPhoneAccess(denied):
            ContentUnavailableView {
                Label("Allow Contacts on Your Phone", systemImage: "person.crop.circle")
            } description: {
                if model.permissionTimedOut {
                    Text("Your phone didn't respond. Unlock it and try again.")
                } else if denied {
                    Text("\(phoneName) stopped sharing its contacts. Allow access on the phone to see them again.")
                } else {
                    Text("Contacts from \(phoneName) appear here after you allow access on the phone.")
                }
            } actions: {
                if model.isRequestingPermission {
                    ProgressView("Waiting for your phone…")
                        .controlSize(.small)
                } else {
                    Button("Send Request to Phone") {
                        Task { await model.requestPermission() }
                    }
                    .buttonStyle(.borderedProminent)
                }
            }
        case .syncing:
            ProgressView("Syncing contacts…")
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        case .empty:
            ContentUnavailableView("No Contacts", systemImage: "person.crop.circle", description: Text("Contacts from your phone appear here."))
        case .list:
            contactList
        }
    }

    private var phoneName: String {
        controller.phone?.name ?? String(localized: "your phone")
    }

    private var contactList: some View {
        @Bindable var model = model
        return List(selection: $model.selection) {
            ForEach(model.sections) { section in
                Section(section.title) {
                    ForEach(section.contacts, id: \.contactId) { contact in
                        ContactRow(contact: contact)
                            .tag(contact.contactId)
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

/// One contact of the list: their picture and name.
private struct ContactRow: View {
    let contact: ContactListItem

    var body: some View {
        HStack(spacing: 10) {
            ContactAvatar(contactId: contact.contactId, name: contact.name, hasPhoto: contact.hasThumbnail, size: 32)
            Text(ContactsModel.name(contact.name))
                .font(.body)
                .foregroundStyle(contact.name.isEmpty ? .secondary : .primary)
                .lineLimit(1)
        }
        .padding(.vertical, 2)
        .accessibilityElement(children: .combine)
    }
}

/// A contact's picture: their photo when the phone has one, else their monogram.
struct ContactAvatar: View {
    let contactId: Int64
    let name: String
    let hasPhoto: Bool
    let size: CGFloat
    @Environment(ContactsModel.self) private var model
    @State private var photo: NSImage?

    var body: some View {
        Monogram(name: name.isEmpty ? nil : name, photo: photo, size: size)
            .onChange(of: contactId) {
                // A reused view shows nothing of the picture it showed before.
                photo = nil
            }
            .task(id: hasPhoto ? contactId : nil) {
                guard hasPhoto else { return }
                photo = await model.image(of: contactId, maxPixels: Int(size * 2))
            }
    }
}
