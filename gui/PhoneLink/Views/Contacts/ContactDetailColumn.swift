import AppKit
import PhoneLinkEngine
import PhoneLinkKit
import SwiftUI

/// The Contacts section's detail column: the selected contact's card, and the failures of the section's
/// actions.
struct ContactDetailColumn: View {
    @Environment(ContactsModel.self) private var model

    var body: some View {
        Group {
            if model.content != .list {
                // The content column already says why there is no list.
                Color.clear
            } else if let card = model.card {
                ContactCardView(card: card)
                    .id(card.contactId)
            } else if model.selection == nil {
                ContentUnavailableView("No Contact Selected", systemImage: "person.crop.circle")
            } else {
                // The card is being read.
                Color.clear
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
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

/// One contact: their picture, name and work over their numbers, addresses, links, dates and notes.
private struct ContactCardView: View {
    let card: ContactCard
    @Environment(ContactsModel.self) private var model
    @Environment(CallControlModel.self) private var calls

    /// The card's widest line: longer lines read poorly in a wide window.
    private static let cardWidth: CGFloat = 560

    var body: some View {
        ScrollView {
            VStack(spacing: 20) {
                header
                if !card.phoneNumbers.isEmpty {
                    ContactFieldGroup {
                        ForEach(Array(card.phoneNumbers.enumerated()), id: \.offset) { index, number in
                            ContactFieldRow(label: ContactsFormat.label(number.label, in: .phone), value: number.shown, divided: index > 0) {
                                Text(number.shown)
                                    .textSelection(.enabled)
                            } trailing: {
                                HStack(spacing: 10) {
                                    if calls.isOffered && calls.hasCallHistory {
                                        CallButton(number: number.shown, iconOnly: true)
                                            .buttonStyle(.borderless)
                                    }
                                    Button {
                                        model.message(number)
                                    } label: {
                                        Label("Message", systemImage: "message")
                                            .labelStyle(.iconOnly)
                                    }
                                    .buttonStyle(.borderless)
                                    .help("Send a message to \(number.shown)")
                                }
                            }
                        }
                    }
                }
                fields(card.emailAddresses, .email) { value in
                    Button(value) { model.email(value) }
                        .buttonStyle(.link)
                        .help("Write an email to \(value)")
                }
                fields(card.postalAddresses, .address) { value in
                    Button(value) { model.showOnMap(value) }
                        .buttonStyle(.link)
                        .multilineTextAlignment(.leading)
                        .help("Show in Maps")
                }
                fields(card.urls, .url) { value in
                    Button(value) { model.open(link: value) }
                        .buttonStyle(.link)
                        .help("Open in your browser")
                }
                fields(card.dates, .date) { value in
                    Text(value)
                        .textSelection(.enabled)
                }
                if !card.notes.isEmpty {
                    ContactFieldGroup {
                        ContactFieldRow(label: String(localized: "notes"), value: card.notes, divided: false) {
                            Text(card.notes)
                                .textSelection(.enabled)
                        }
                    }
                }
            }
            // The card's text wraps within this width; it must not size itself to its lines, which
            // would widen the window.
            .frame(maxWidth: Self.cardWidth)
            .padding(24)
            .frame(maxWidth: .infinity)
        }
    }

    /// The picture, the name, and under it the nickname and the work.
    private var header: some View {
        VStack(spacing: 6) {
            ContactAvatar(contactId: card.contactId, name: card.name, hasPhoto: card.hasThumbnail, size: 88)
                .padding(.bottom, 4)
            Text(ContactsModel.name(card.name))
                .font(.title2.weight(.semibold))
                .foregroundStyle(card.name.isEmpty ? .secondary : .primary)
                .multilineTextAlignment(.center)
                .textSelection(.enabled)
            if !card.nickname.isEmpty && card.nickname != card.name {
                Text("“\(card.nickname)”")
                    .foregroundStyle(.secondary)
                    .textSelection(.enabled)
            }
            if let subtitle = ContactsFormat.subtitle(card) {
                Text(subtitle)
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
                    .textSelection(.enabled)
            }
        }
        .frame(maxWidth: .infinity)
    }

    /// The fields of one kind in one group, each with its label; nothing when there are none.
    @ViewBuilder
    private func fields<Value: View>(
        _ fields: [ContactCardField],
        _ section: ContactFieldSection,
        @ViewBuilder value: @escaping (String) -> Value
    ) -> some View {
        if !fields.isEmpty {
            ContactFieldGroup {
                ForEach(Array(fields.enumerated()), id: \.offset) { index, field in
                    ContactFieldRow(label: ContactsFormat.label(field.label, in: section), value: field.value, divided: index > 0) {
                        value(field.value)
                    }
                }
            }
        }
    }
}

/// A group of fields on the card's rounded background.
private struct ContactFieldGroup<Content: View>: View {
    @ViewBuilder let content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            content
        }
        .padding(.horizontal, 14)
        .background(.quinary, in: RoundedRectangle(cornerRadius: 10))
    }
}

/// One field: its label over its value, an action at the trailing edge, and Copy in its menu; a
/// divider above it when it follows another field of its group.
private struct ContactFieldRow<Value: View, Trailing: View>: View {
    let label: String
    let value: String
    let divided: Bool
    let content: Value
    let trailing: Trailing

    init(
        label: String,
        value: String,
        divided: Bool,
        @ViewBuilder content: () -> Value,
        @ViewBuilder trailing: () -> Trailing = { EmptyView() }
    ) {
        self.label = label
        self.value = value
        self.divided = divided
        self.content = content()
        self.trailing = trailing()
    }

    var body: some View {
        HStack(alignment: .center, spacing: 8) {
            VStack(alignment: .leading, spacing: 2) {
                Text(label)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                content
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            trailing
        }
        .padding(.vertical, 8)
        .overlay(alignment: .top) {
            if divided {
                Divider()
            }
        }
        .contentShape(Rectangle())
        .contextMenu {
            Button("Copy") {
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(value, forType: .string)
            }
        }
    }
}
