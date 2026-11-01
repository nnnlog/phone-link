import Foundation
import PhoneLinkEngine

/// How the new-message composer reads addresses the user typed.
enum MessagesAddress {
    /// An address the user may enter as it is: a phone number (dial characters only, at least three
    /// digits) or an email address.
    static func isPlausible(_ text: String) -> Bool {
        let typed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        if typed.contains("@") {
            let sides = typed.split(separator: "@", omittingEmptySubsequences: false)
            return sides.count == 2 && !sides[0].isEmpty && sides[1].contains(".") && !typed.contains(" ")
        }

        let dial = CharacterSet(charactersIn: "+0123456789 -().#*")
        return typed.unicodeScalars.allSatisfy(dial.contains) && digits(typed).count >= 3
    }

    /// Whether two addresses name the same recipient. The phone stores numbers in its own form (often
    /// E.164) while the user types the national one, so numbers match when, with leading zeros (a trunk
    /// prefix) dropped, the shorter of at least seven digits ends the longer — the longer may carry the
    /// country code. Other addresses match case-insensitively.
    static func same(_ a: String, _ b: String) -> Bool {
        let da = digits(a)
        let db = digits(b)

        if a.contains("@") || b.contains("@") || da.isEmpty || db.isEmpty {
            let trim = { (s: String) in s.trimmingCharacters(in: .whitespaces) }
            return trim(a).caseInsensitiveCompare(trim(b)) == .orderedSame
        }

        if da == db { return true }
        let ta = String(da.drop { $0 == "0" })
        let tb = String(db.drop { $0 == "0" })
        let (short, long) = ta.count <= tb.count ? (ta, tb) : (tb, ta)
        return short.count >= 7 && long.hasSuffix(short)
    }

    /// Whether two lists of addresses name the same people, in any order.
    static func sameRecipients(_ a: [String], _ b: [String]) -> Bool {
        guard a.count == b.count else { return false }
        var unmatched = b
        for address in a {
            guard let match = unmatched.firstIndex(where: { same(address, $0) }) else { return false }
            unmatched.remove(at: match)
        }
        return true
    }

    private static func digits(_ s: String) -> String {
        String(s.filter { $0.isASCII && $0.isNumber })
    }
}

enum MessagesUnsent {
    /// The messages sent to new recipients, one group per set of recipients (`MessagesAddress.sameRecipients`),
    /// newest group first; each group's messages oldest first, as the engine lists them.
    static func groups(_ messages: [NewConversationMessage]) -> [MessagesUnsentConversation] {
        var groups: [MessagesUnsentConversation] = []
        for sent in messages {
            let addresses = sent.recipients.map(\.address)
            if let index = groups.firstIndex(where: { MessagesAddress.sameRecipients($0.addresses, addresses) }) {
                groups[index].messages.append(sent.message)
            } else {
                groups.append(MessagesUnsentConversation(recipients: sent.recipients, messages: [sent.message]))
            }
        }
        return groups.sorted { ($0.latest?.timestampMs ?? 0) > ($1.latest?.timestampMs ?? 0) }
    }
}

enum MessagesTextLimit {
    /// `text` cut to at most `maxUTF16` UTF-16 code units — the engine's count — at a character boundary.
    static func clamp(_ text: String, maxUTF16: Int?) -> String {
        guard let maxUTF16, text.utf16.count > maxUTF16 else { return text }
        var kept = ""
        var count = 0
        for character in text {
            count += character.utf16.count
            if count > maxUTF16 { break }
            kept.append(character)
        }
        return kept
    }
}

enum MessagesTranscript {
    /// The transcript of `messages` (oldest first, as the engine lists them): a day and time before the
    /// first message and after every gap of over an hour, and in a group the sender's name above each
    /// received message that starts a run of theirs.
    static func rows(_ messages: [Message], isGroup: Bool, time: MessagesTimeFormatter) -> [MessagesTranscriptRow] {
        var rows: [MessagesTranscriptRow] = []
        var previous: Message?
        for message in messages {
            let startsBlock = previous.map { message.timestampMs - $0.timestampMs > 3_600_000 } ?? true
            let separator = startsBlock ? time.separator(ms: message.timestampMs) : nil
            let continuesRun = !startsBlock && previous.map {
                $0.incoming && $0.groupEvent == nil && $0.contact.address == message.contact.address
            } == true
            let showsSender = isGroup && message.incoming && message.groupEvent == nil && !continuesRun

            rows.append(MessagesTranscriptRow(
                message: message,
                separator: separator,
                sender: showsSender ? MessagesFormat.name(message.contact) : nil
            ))
            previous = message
        }
        return rows
    }
}
