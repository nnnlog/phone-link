import Foundation
import PhoneLinkEngine
import UniformTypeIdentifiers

/// Names times the way Messages does — the list by how long ago, the transcript with the time — on an
/// injectable clock and calendar (the calendar's locale and time zone format the text).
public struct MessagesTimeFormatter: Sendable {
    /// How far a day lies behind today.
    public enum Bucket: Equatable, Sendable {
        case today
        case yesterday
        /// Two to six days ago.
        case thisWeek
        /// A week ago or more, or a day still ahead (a phone clock running fast).
        case earlier
    }

    public var now: @Sendable () -> Date
    public var calendar: Calendar

    public init(now: @escaping @Sendable () -> Date = { Date() }, calendar: Calendar = .autoupdatingCurrent) {
        self.now = now
        self.calendar = calendar
    }

    public func bucket(_ date: Date) -> Bucket {
        let today = calendar.startOfDay(for: now())
        let day = calendar.startOfDay(for: date)
        switch calendar.dateComponents([.day], from: day, to: today).day {
        case 0: return .today
        case 1: return .yesterday
        case .some(2...6): return .thisWeek
        default: return .earlier
        }
    }

    /// The list's time: the time today, "Yesterday", the weekday within the week, else the date.
    public func listTime(ms: Int64) -> String {
        let date = Self.date(ms: ms)
        switch bucket(date) {
        case .today: return date.formatted(style(time: .shortened))
        case .yesterday: return String(localized: "Yesterday")
        case .thisWeek: return date.formatted(style().weekday(.wide))
        case .earlier: return date.formatted(style().year(.twoDigits).month(.defaultDigits).day(.defaultDigits))
        }
    }

    /// A transcript separator: "Today 1:52 PM", "Yesterday 9:41 AM", "Monday 3:00 PM", or the date
    /// with the time.
    public func separator(ms: Int64) -> String {
        let date = Self.date(ms: ms)
        let time = date.formatted(style(time: .shortened))
        switch bucket(date) {
        case .today: return String(localized: "Today \(time)")
        case .yesterday: return String(localized: "Yesterday \(time)")
        case .thisWeek: return "\(date.formatted(style().weekday(.wide))) \(time)"
        case .earlier: return date.formatted(style(date: .abbreviated, time: .shortened))
        }
    }

    static func date(ms: Int64) -> Date {
        Date(timeIntervalSince1970: TimeInterval(ms) / 1000)
    }

    private func style(date: Date.FormatStyle.DateStyle? = nil, time: Date.FormatStyle.TimeStyle? = nil) -> Date.FormatStyle {
        Date.FormatStyle(
            date: date,
            time: time,
            locale: calendar.locale ?? .autoupdatingCurrent,
            calendar: calendar,
            timeZone: calendar.timeZone
        )
    }
}

/// The Messages section's texts for what the engine reports.
public enum MessagesFormat {
    /// A person: their contact name, else their address.
    public static func name(_ contact: MessageContact) -> String {
        contact.name ?? contact.address
    }

    /// A conversation's title: its recipients' names (else addresses) joined with ", ".
    public static func title(_ conversation: Conversation) -> String {
        let names = conversation.recipients.map(name)
        return names.isEmpty ? String(localized: "Unknown") : names.joined(separator: ", ")
    }

    /// The line under the title in a conversation's header: the number of a one-person conversation
    /// whose title is the contact's name.
    public static func subtitle(_ conversation: Conversation) -> String? {
        guard conversation.recipients.count == 1, let only = conversation.recipients.first, only.name != nil else { return nil }
        return only.address
    }

    /// The list's preview of the latest message: its text on one line, "Attachment" for a message with
    /// attachments only, and in a group the sender's name before a received one.
    public static func preview(_ conversation: Conversation) -> String {
        guard let latest = conversation.preview else { return "" }
        let text = line(latest.body, attachment: latest.attachment)
        guard conversation.isGroup, let sender = latest.from else { return text }
        return "\(name(sender)): \(text)"
    }

    /// The title of messages sent to new recipients: "To: " and their names.
    public static func title(_ unsent: MessagesUnsentConversation) -> String {
        let names = unsent.recipients.map(name).joined(separator: ", ")
        return String(localized: "To: \(names)")
    }

    /// The list's line under it: how the newest one's send went, or its text once the phone took it.
    public static func preview(_ unsent: MessagesUnsentConversation) -> String {
        guard let latest = unsent.latest else { return "" }
        switch latest.sendState {
        case let .failed(reason): return notDelivered(reason)
        case .sending: return String(localized: "Sending…")
        case .sent, nil: return line(latest.body, attachment: !latest.parts.isEmpty)
        }
    }

    /// The list's line under the new message being written: its recipients, else its text.
    public static func newMessagePreview(recipients: [MessagesRecipient], text: String) -> String {
        guard recipients.isEmpty else {
            let names = recipients.map(\.title).joined(separator: ", ")
            return String(localized: "To: \(names)")
        }
        return line(text, attachment: false)
    }

    /// A group event as the transcript shows it: what the phone wrote about it, else what happened.
    public static func groupEvent(_ message: Message) -> String {
        guard message.body.isEmpty, let event = message.groupEvent else { return message.body }
        let who = message.contact.address.isEmpty ? String(localized: "Someone") : name(message.contact)
        switch event {
        case .joinedSelf: return String(localized: "You joined the conversation")
        case .joined: return String(localized: "\(who) joined the conversation")
        case .leftSelf: return String(localized: "You left the conversation")
        case .left: return String(localized: "\(who) left the conversation")
        case .allParticipantsLeft: return String(localized: "Everyone left the conversation")
        case .leaderChanged: return String(localized: "The group leader changed")
        case .renamedByLeader: return String(localized: "The group was renamed")
        case .removedByLeader: return String(localized: "\(who) was removed from the conversation")
        }
    }

    /// Under a message the phone holds and has not delivered: that it is still being sent, that it
    /// failed, or that it is a draft. `nil` for a received or sent message, and for one sent from this
    /// Mac (its own state shows instead).
    public static func phoneStatus(_ message: Message) -> String? {
        guard message.sendState == nil else { return nil }
        switch message.mailbox {
        case .outbox, .queued: return String(localized: "Sending…")
        case .failed: return String(localized: "Not Delivered")
        case .draft: return String(localized: "Draft")
        case .inbox, .sent, .unknown: return nil
        }
    }

    /// Text on one line; "Attachment" when there is none but an attachment.
    private static func line(_ text: String, attachment: Bool) -> String {
        text.isEmpty && attachment ? String(localized: "Attachment") : text.replacingOccurrences(of: "\n", with: " ")
    }

    /// Under a message the phone could not send: "Not Delivered", with the reason when the phone gave one.
    public static func notDelivered(_ reason: SendFailure) -> String {
        guard let why = failure(reason) else { return String(localized: "Not Delivered") }
        return String(localized: "Not Delivered · \(why)")
    }

    /// Why the phone could not send; `nil` when it did not say.
    public static func failure(_ reason: SendFailure) -> String? {
        switch reason {
        case .airplaneMode: String(localized: "Airplane mode is on")
        case .noSim: String(localized: "No SIM card")
        case .messageSizeExceeded, .mmsTransactionSizeExceeded: String(localized: "The message is too large")
        case .mmsNotSupported: String(localized: "MMS isn't supported")
        case .mmsDisabled: String(localized: "MMS is turned off")
        case .mmsGroupMessageDisabled: String(localized: "Group messages are turned off")
        case .mmsTooManyRecipients: String(localized: "Too many recipients")
        case .mmsSubjectSizeExceeded: String(localized: "The subject is too long")
        case .mmsAudioNotAllowed: String(localized: "Audio can't be sent")
        case .actionRequired: String(localized: "Check your phone")
        case .empty: String(localized: "The message is empty")
        case .rcsSendUnavailable: String(localized: "Chat isn't available")
        case .phoneTimeout: String(localized: "Your phone didn't respond")
        case .radioOff: String(localized: "Mobile network is off")
        case .noService: String(localized: "No service")
        case .noRecipients: String(localized: "No recipients")
        case .bluetoothNotConnected: String(localized: "Your phone isn't connected over Bluetooth")
        case .unknown, .other: nil
        }
    }

    /// Why a phone that sends only over Bluetooth cannot send now (the `SendRequiresProximity`
    /// gate); `nil` while it is connected or does not need Bluetooth.
    public static func bluetoothNotice(_ proximity: ProximityState?) -> String? {
        switch proximity {
        case nil, .connected: nil
        case .bluetoothOff: String(localized: "Bluetooth is off. Turn it on to send messages from this Mac.")
        case .phoneNotPaired: String(localized: "Your phone isn't paired with this Mac. Pair it in System Settings › Bluetooth to send messages.")
        case .pairedPhoneUnavailable: String(localized: "Your phone isn't in Bluetooth range of this Mac.")
        case .notConnected: String(localized: "Your phone isn't connected to this Mac over Bluetooth.")
        }
    }

    /// An attachment's row: its kind and, when known, its size ("Image · 1.4 MB").
    public static func attachmentTitle(_ part: MessagePart) -> String {
        let kind = MessagesAttachmentKind(contentType: part.contentType).title
        guard part.size > 0 else { return kind }
        return "\(kind) · \(ByteCountFormatter.string(fromByteCount: part.size, countStyle: .file))"
    }

    /// A SIM's name, else its slot.
    public static func simName(_ sim: SimSubscription) -> String {
        sim.name.isEmpty ? String(localized: "SIM \(sim.simSlotIndex + 1)") : sim.name
    }

    /// A suggestion's second line: the number's kind and the number.
    public static func numberLine(_ suggestion: ContactSuggestion) -> String {
        let kind: String? = switch suggestion.kind {
        case .mobile: String(localized: "Mobile", comment: "Phone number type")
        case .home: String(localized: "Home", comment: "Phone number type")
        case .work: String(localized: "Work", comment: "Phone number type")
        case .workMobile: String(localized: "Work Mobile", comment: "Phone number type")
        case .main: String(localized: "Main", comment: "Phone number type")
        case .school: String(localized: "School", comment: "Phone number type")
        case .other: String(localized: "Other", comment: "Phone number type")
        case .custom: suggestion.label.isEmpty ? nil : suggestion.label
        case .unknown: nil
        }
        return kind.map { "\($0) · \(suggestion.number)" } ?? suggestion.number
    }
}

/// What an attachment is, by its MIME type.
public enum MessagesAttachmentKind: Equatable, Sendable {
    case image
    case video
    case audio
    case contact
    case text
    case other

    public init(contentType: String) {
        let mime = contentType.split(separator: ";").first.map { $0.trimmingCharacters(in: .whitespaces).lowercased() } ?? ""
        guard let type = UTType(mimeType: mime) else {
            self = .other
            return
        }

        self = if type.conforms(to: .image) {
            .image
        } else if type.conforms(to: .movie) {
            .video
        } else if type.conforms(to: .audio) {
            .audio
        } else if type.conforms(to: .vCard) {
            .contact
        } else if type.conforms(to: .text) {
            .text
        } else {
            .other
        }
    }

    public var title: String {
        switch self {
        case .image: String(localized: "Image", comment: "Attachment type")
        case .video: String(localized: "Video", comment: "Attachment type")
        case .audio: String(localized: "Audio", comment: "Attachment type")
        case .contact: String(localized: "Contact", comment: "Attachment type")
        case .text: String(localized: "Text", comment: "Attachment type")
        case .other: String(localized: "Attachment", comment: "Attachment type")
        }
    }
}
