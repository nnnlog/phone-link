import PhoneLinkEngine
import PhoneLinkKit
import SwiftUI

/// The "To:" line of the new message — a field that turns names and numbers into recipients, with the
/// phone's contacts suggested below it — and, once messages were sent to them, the fixed recipients.
/// Escape in the field closes an empty new message.
struct MessagesRecipientHeader: View {
    @FocusState.Binding var fieldFocused: Bool
    @Environment(MessagesModel.self) private var model

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            MessagesRecipientFlow(spacing: 4, lastFills: model.canAddRecipient) {
                Text("To:")
                    .foregroundStyle(.secondary)
                ForEach(model.recipients) { recipient in
                    MessagesRecipientToken(
                        recipient: recipient,
                        onRemove: model.recipientsLocked ? nil : { model.removeRecipient(recipient) }
                    )
                }
                if model.canAddRecipient {
                    TextField(
                        model.recipients.isEmpty ? String(localized: "Name or Number") : "",
                        text: Binding(get: { model.recipientQuery }, set: { model.setRecipientQuery($0) })
                    )
                    .textFieldStyle(.plain)
                    .focused($fieldFocused)
                    .onSubmit { model.submitRecipientQuery() }
                    .onExitCommand { model.cancelNewMessage() }
                    .accessibilityLabel("To")
                }
            }
            .padding(.horizontal, 20)
            .padding(.vertical, 10)
            if model.detail == .newMessage, let notice = model.contactsNotice {
                contactsLine(notice)
            }
            Divider()
        }
        .background(.bar)
    }

    private func contactsLine(_ notice: MessagesContactsNotice) -> some View {
        HStack(spacing: 8) {
            switch notice {
            case .allowOnPhone:
                Text("Allow access to your phone's contacts to search them")
                    .foregroundStyle(.secondary)
                Spacer(minLength: 8)
                Button("Send Request to Phone") {
                    Task { await model.requestContactsPermission() }
                }
                .disabled(model.isRequestingContacts)
            case .turnOn:
                Text("Turn on your phone's contacts on this Mac to search them")
                    .foregroundStyle(.secondary)
                Spacer(minLength: 8)
                Button("Turn On") { model.turnOnContacts() }
            }
        }
        .font(.callout)
        .controlSize(.small)
        .padding(.horizontal, 20)
        .padding(.bottom, 8)
    }
}

/// The contacts matching what is typed in "To:"; a click adds one as a recipient.
struct MessagesSuggestionList: View {
    /// After a suggestion was added: the field takes the focus back.
    let onAdd: () -> Void
    @Environment(MessagesModel.self) private var model

    var body: some View {
        List {
            ForEach(Array(model.suggestions.enumerated()), id: \.offset) { _, suggestion in
                Button {
                    model.addRecipient(suggestion)
                    onAdd()
                } label: {
                    HStack(spacing: 10) {
                        if suggestion.hasThumbnail {
                            MessagesAsyncImage(picture: .contact(suggestion.contactId), maxPixels: 96) { photo in
                                Monogram(name: suggestion.name, photo: photo, size: 28)
                            }
                        } else {
                            Monogram(name: suggestion.name, size: 28)
                        }
                        VStack(alignment: .leading, spacing: 1) {
                            Text(suggestion.name)
                            Text(MessagesFormat.numberLine(suggestion))
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                        Spacer(minLength: 0)
                    }
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }
        }
    }
}

/// A recipient in the "To:" field, removable until messages were sent to it.
private struct MessagesRecipientToken: View {
    let recipient: MessagesRecipient
    let onRemove: (() -> Void)?

    var body: some View {
        HStack(spacing: 2) {
            Text(recipient.title)
                .lineLimit(1)
            if let onRemove {
                Button(action: onRemove) {
                    Label("Remove \(recipient.title)", systemImage: "xmark.circle.fill")
                }
                .labelStyle(.iconOnly)
                .buttonStyle(.borderless)
                .foregroundStyle(.secondary)
            }
        }
        .padding(.leading, 8)
        .padding(.trailing, onRemove == nil ? 8 : 4)
        .padding(.vertical, 2)
        .background(.quaternary, in: Capsule())
        .help(recipient.address)
    }
}

/// Lays its views out in lines that wrap, each line centered vertically. With `lastFills` the last view —
/// the field the user types in — takes the rest of its line, or a line of its own when less than a
/// third is left.
private struct MessagesRecipientFlow: Layout {
    var spacing: CGFloat
    var lastFills: Bool

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let width = proposal.width ?? .infinity
        let lines = lines(for: subviews, width: width)
        let height = lines.map(\.height).reduce(0, +) + spacing * CGFloat(max(lines.count - 1, 0))
        let widest = lines.map(\.width).max() ?? 0
        return CGSize(width: proposal.width ?? widest, height: height)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        var y = bounds.minY
        for line in lines(for: subviews, width: bounds.width) {
            var x = bounds.minX
            for item in line.items {
                subviews[item.index].place(
                    at: CGPoint(x: x, y: y + (line.height - item.size.height) / 2),
                    proposal: ProposedViewSize(item.size)
                )
                x += item.size.width + spacing
            }
            y += line.height + spacing
        }
    }

    private struct Line {
        var items: [(index: Int, size: CGSize)] = []
        var width: CGFloat = 0
        var height: CGFloat = 0
    }

    private func lines(for subviews: Subviews, width: CGFloat) -> [Line] {
        var lines = [Line()]
        for index in subviews.indices {
            var size = subviews[index].sizeThatFits(.unspecified)
            size.width = min(size.width, width)
            let isField = lastFills && index == subviews.indices.last

            if let line = lines.last, !line.items.isEmpty {
                let start = line.width + spacing
                let fits = isField ? width - start >= width / 3 : start + size.width <= width
                if !fits {
                    lines.append(Line())
                }
            }

            var line = lines.removeLast()
            let start = line.items.isEmpty ? 0 : line.width + spacing
            if isField, width.isFinite {
                size.width = max(width - start, 0)
                size.height = subviews[index].sizeThatFits(ProposedViewSize(width: size.width, height: nil)).height
            }

            line.items.append((index, size))
            line.width = start + size.width
            line.height = max(line.height, size.height)
            lines.append(line)
        }
        return lines
    }
}
