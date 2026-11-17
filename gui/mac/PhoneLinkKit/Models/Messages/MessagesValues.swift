import Foundation
import PhoneLinkEngine
import UniformTypeIdentifiers

/// What the detail column shows.
public enum MessagesDetail: Hashable, Sendable {
    /// A conversation of the phone.
    case conversation(Int64)
    /// The new message being written: its recipients can still change.
    case newMessage
    /// What was sent from this Mac to these recipients before the phone created their conversation:
    /// the sends with their state and retry, and a composer that sends to the same recipients.
    case unsent([MessagesRecipient])
}

/// A row of the conversation list.
public enum MessagesListItem: Hashable, Sendable {
    /// The new message being written.
    case newMessage
    /// Messages sent to new recipients, by the group's addresses (`MessagesUnsentConversation.id`).
    case unsent([String])
    case conversation(Int64)
}

/// Where the composer sends, which keys its draft: a conversation, the new message, or the
/// recipients of an unsent conversation (their addresses).
public enum MessagesComposerTarget: Hashable, Sendable {
    case conversation(Int64)
    case newMessage
    case recipients([String])
}

/// Messages sent from this Mac to the same people before the phone created a conversation for them
/// (or while their send failed): one list row, until their conversation exists — then they show in it.
public struct MessagesUnsentConversation: Identifiable, Equatable, Sendable {
    /// The recipients of its first message, with their contacts.
    public let recipients: [MessageContact]
    /// Oldest first.
    public internal(set) var messages: [Message]

    public var addresses: [String] { recipients.map(\.address) }
    public var id: [String] { addresses }
    /// The newest message.
    public var latest: Message? { messages.last }
}

/// An image the user added to a draft: picked, pasted or dropped.
public struct MessagesAttachment: Identifiable, Equatable, Sendable {
    public let id: UUID
    /// The file name; empty for pasted or dropped image data.
    public let name: String
    /// The MIME type when known; the engine detects it from the bytes otherwise.
    public let contentType: String?
    public let data: Data

    public init(name: String, contentType: String?, data: Data) {
        id = UUID()
        self.name = name
        self.contentType = contentType
        self.data = data
    }

    /// An image file, typed by its extension.
    public init(contentsOf url: URL) throws {
        self.init(
            name: url.lastPathComponent,
            contentType: UTType(filenameExtension: url.pathExtension)?.preferredMIMEType,
            data: try Data(contentsOf: url)
        )
    }

    /// A message carries images only (composition rule 5); without a type the engine decides.
    var isImage: Bool {
        guard let contentType else { return true }
        return UTType(mimeType: contentType)?.conforms(to: .image) == true
    }

    var engineAttachment: MessageAttachment {
        MessageAttachment(name: name, contentType: contentType, bytes: data)
    }
}

/// The unsent text and images of one composer target.
public struct MessagesDraft: Equatable, Sendable {
    public var text = ""
    public var attachments: [MessagesAttachment] = []

    public init() {}

    var isEmpty: Bool {
        text.isEmpty && attachments.isEmpty
    }

    /// Something worth sending: text other than white space, or an image.
    var hasContent: Bool {
        !attachments.isEmpty || !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }
}

/// A recipient of a new message: the address as the user entered or picked it (the engine sends it
/// unnormalised).
public struct MessagesRecipient: Identifiable, Hashable, Sendable {
    public let address: String
    /// The contact's name, for a recipient picked from the suggestions or found in the contacts.
    public let name: String?

    public init(address: String, name: String?) {
        self.address = address
        self.name = name
    }

    public var id: String { address }
    public var title: String { name ?? address }
}

/// The failure of an action the user started.
public struct MessagesAlert: Identifiable, Equatable, Sendable {
    public let id = UUID()
    public let title: String
    public let message: String

    public init(title: String, message: String) {
        self.title = title
        self.message = message
    }
}

/// What the conversation list shows.
public enum MessagesListStatus: Equatable, Sendable {
    /// "Show messages on this Mac" is off.
    case turnedOff
    /// The phone has not allowed messages on this Mac.
    case permissionNeeded
    /// Nothing arrived yet and the sync is under way.
    case syncing
    /// No conversations.
    case empty
    case conversations
}

/// How the new-message composer offers contacts it cannot search yet.
public enum MessagesContactsNotice: Equatable, Sendable {
    /// The phone has not allowed contacts: ask it.
    case allowOnPhone
    /// "Use my phone's contacts" is off on this Mac.
    case turnOn
}

/// What the composer allows for its target.
public struct MessagesComposerState: Equatable, Sendable {
    public var isEnabled: Bool
    /// The text field's placeholder; for a conversation that cannot be replied to, the reason.
    public var placeholder: String
    /// Images are offered here.
    public var canAttach: Bool
    /// The carrier's text limit in UTF-16 code units (as the engine counts); `nil` is unlimited.
    public var maxTextLength: Int?
}

/// A message's identity in a transcript: ids are unique per kind only.
public struct MessagesMessageKey: Hashable, Sendable {
    public let kind: MessageType
    public let id: Int64
}

/// One message of a transcript with what the transcript shows around it.
public struct MessagesTranscriptRow: Identifiable, Equatable, Sendable {
    public let message: Message
    /// The day and time above it: before the first message and after a gap of over an hour.
    public let separator: String?
    /// In a group, the sender's name above a received message that starts a run of theirs.
    public let sender: String?

    public var id: MessagesMessageKey {
        MessagesMessageKey(kind: message.kind, id: message.id)
    }
}

/// A picture the section shows, read and decoded off the main actor.
public enum MessagesPicture: Hashable, Sendable {
    /// A message's image, by its part and size (a part whose bytes change is another picture).
    case part(MessagePartRef, size: Int64)
    /// A contact's photo.
    case contact(Int64)
}
