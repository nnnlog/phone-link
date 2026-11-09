import PhoneLinkEngine
import PhoneLinkKit
import SwiftUI

/// The Contacts section's content column: the phone's contacts under their index letters, filtered by
/// the search — or the state that stands in for them: contacts off on this Mac, not allowed on the
/// phone, the first sync, or none at all.
struct ContactListColumn: View {
    @Environment(ContactsModel.self) private var model
    @Environment(EngineController.self) private var controller
    @Environment(AppNavigator.self) private var navigator

    var body: some View {
        @Bindable var model = model
        VStack(spacing: 0) {
            // Inline Search Bar
            HStack(spacing: 6) {
                Image(systemName: "magnifyingglass")
                    .foregroundStyle(.secondary)
                    .font(.system(size: 13))
                TextField("Search Contacts", text: $model.searchText)
                    .textFieldStyle(.plain)
                    .font(.system(size: 13))
                if model.isSyncing && model.content == .list {
                    ProgressView()
                        .controlSize(.small)
                }
                if !model.searchText.isEmpty {
                    Button {
                        model.searchText = ""
                    } label: {
                        Image(systemName: "xmark.circle.fill")
                            .foregroundStyle(.secondary)
                            .font(.system(size: 12))
                    }
                    .buttonStyle(.plain)
                }
            }
            .padding(.horizontal, 8)
            .padding(.vertical, 6)
            .background(Color.primary.opacity(0.06), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
            .padding(.horizontal, 12)
            .padding(.vertical, 8)

            Divider()

            content
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        // A call's Contact button selects that contact, with the search cleared so the list shows it.
        .onChange(of: navigator.contactRequest, initial: true) { _, id in
            guard let id else { return }
            model.searchText = ""
            model.selection = id
            navigator.contactRequest = nil
        }
        .navigationSplitViewColumnWidth(min: 260, ideal: 300, max: 420)
    }

    @ViewBuilder
    private var content: some View {
        switch model.content {
        case .nothing:
            Color.clear
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        case .turnedOff:
            ContentUnavailableView {
                Label("Contacts Are Off on This Mac", systemImage: "person.crop.circle")
            } description: {
                Text("Turn on contacts to see your phone's contacts on this Mac.")
            } actions: {
                Button("Turn On") { model.turnOn() }
                    .buttonStyle(.borderedProminent)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
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
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        case .syncing:
            ProgressView("Syncing contacts…")
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        case .empty:
            ContentUnavailableView("No Contacts", systemImage: "person.crop.circle", description: Text("Contacts from your phone appear here."))
                .frame(maxWidth: .infinity, maxHeight: .infinity)
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
