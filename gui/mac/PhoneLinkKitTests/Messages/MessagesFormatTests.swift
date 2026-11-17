import Foundation
import PhoneLinkEngine
import Testing
@testable import PhoneLinkKit

private let calendar: Calendar = {
    var calendar = Calendar(identifier: .gregorian)
    calendar.timeZone = TimeZone(identifier: "UTC")!
    calendar.locale = Locale(identifier: "en_US")
    return calendar
}()

/// Wednesday, 21 October 2026, 14:00 UTC.
private let now = calendar.date(from: DateComponents(year: 2026, month: 10, day: 21, hour: 14))!
private let formatter = MessagesTimeFormatter(now: { now }, calendar: calendar)

private func ms(_ month: Int, _ day: Int, _ hour: Int, _ minute: Int = 0) -> Int64 {
    let date = calendar.date(from: DateComponents(year: 2026, month: month, day: day, hour: hour, minute: minute))!
    return Int64(date.timeIntervalSince1970 * 1000)
}

private func person(_ address: String, _ name: String? = nil) -> MessageContact {
    MessageContact(address: address, name: name, contactId: nil, photoContactId: nil)
}

private func conversation(_ people: [MessageContact], preview: ConversationPreviewLine?) -> Conversation {
    Conversation(
        threadId: 1,
        recipients: people,
        isGroup: people.count > 1,
        latestMs: 0,
        preview: preview,
        unread: 0,
        muted: false,
        hidden: false,
        canReply: true,
        attachmentsAllowed: true,
        activeSubscription: -1,
        maxTextLength: nil,
        maxRecipients: nil
    )
}

private func message(
    _ id: Int64,
    at timestamp: Int64,
    incoming: Bool = true,
    from contact: MessageContact,
    body: String? = nil,
    mailbox: MessageMailbox? = nil,
    groupEvent: MessageGroupEvent? = nil,
    kind: MessageType = .rcsChat,
    parts: [MessagePart] = [],
    sendState: MessageSendState? = nil
) -> Message {
    Message(
        kind: kind,
        id: id,
        timestampMs: timestamp,
        incoming: incoming,
        mailbox: mailbox ?? (incoming ? .inbox : .sent),
        contact: contact,
        body: body ?? "text \(id)",
        subject: "",
        read: true,
        subscriptionId: -1,
        groupEvent: groupEvent,
        parts: parts,
        sendState: sendState
    )
}

// MARK: - Times

@Test func timeBucketsCountDaysBackFromToday() {
    func bucket(_ month: Int, _ day: Int, _ hour: Int) -> MessagesTimeFormatter.Bucket {
        formatter.bucket(MessagesTimeFormatter.date(ms: ms(month, day, hour)))
    }

    #expect(bucket(10, 21, 0) == .today)
    #expect(bucket(10, 21, 23) == .today)
    #expect(bucket(10, 20, 23) == .yesterday)
    #expect(bucket(10, 20, 0) == .yesterday)
    #expect(bucket(10, 19, 12) == .thisWeek)
    #expect(bucket(10, 15, 0) == .thisWeek)
    #expect(bucket(10, 14, 23) == .earlier)
    #expect(bucket(10, 22, 1) == .earlier, "a day ahead of today")
}

@Test func theListNamesTimesByHowLongAgo() {
    let today = formatter.listTime(ms: ms(10, 21, 9, 5))
    #expect(today.hasPrefix("9:05"))
    #expect(today.hasSuffix("AM"))
    #expect(formatter.listTime(ms: ms(10, 20, 22)) == "Yesterday")
    #expect(formatter.listTime(ms: ms(10, 19, 8)) == "Monday")
    #expect(formatter.listTime(ms: ms(10, 14, 8)) == "10/14/26")
}

@Test func separatorsCarryTheDayAndTheTime() {
    #expect(formatter.separator(ms: ms(10, 21, 13, 52)).hasPrefix("Today 1:52"))
    #expect(formatter.separator(ms: ms(10, 20, 9, 41)).hasPrefix("Yesterday 9:41"))
    #expect(formatter.separator(ms: ms(10, 19, 15)).hasPrefix("Monday 3:00"))

    let earlier = formatter.separator(ms: ms(10, 2, 15))
    #expect(earlier.hasPrefix("Oct 2, 2026"))
    #expect(earlier.contains("3:00"))
}

// MARK: - Transcript

@Test func transcriptsSeparateStretchesAndNameGroupSenders() {
    let alex = person("+15550101", "Alex Kim")
    let minji = person("+821055556666", "김민지")
    let me = person("+15550199")

    let rows = MessagesTranscript.rows([
        message(1, at: ms(10, 21, 9), from: alex),
        message(2, at: ms(10, 21, 9, 30), from: alex),
        message(3, at: ms(10, 21, 9, 31), from: minji),
        message(4, at: ms(10, 21, 9, 32), incoming: false, from: me),
        message(5, at: ms(10, 21, 9, 33), from: minji),
        message(6, at: ms(10, 21, 9, 34), from: minji, groupEvent: .joined),
        message(7, at: ms(10, 21, 11), from: minji),
    ], isGroup: true, time: formatter)

    #expect(rows.map { $0.separator != nil } == [true, false, false, false, false, false, true])
    #expect(rows.map(\.sender) == ["Alex Kim", nil, "김민지", nil, "김민지", nil, "김민지"])
    #expect(rows.map(\.id) == (1...7).map { MessagesMessageKey(kind: .rcsChat, id: Int64($0)) })

    let single = MessagesTranscript.rows([message(1, at: ms(10, 21, 9), from: alex)], isGroup: false, time: formatter)
    #expect(single.first?.sender == nil, "one-to-one conversations name nobody")
}

// MARK: - Texts

@Test func previewsShowTheLatestMessage() {
    let alex = person("+15550101", "Alex Kim")
    let stranger = person("+15550123")
    func preview(_ people: [MessageContact], _ line: ConversationPreviewLine?) -> String {
        MessagesFormat.preview(conversation(people, preview: line))
    }

    #expect(preview([alex], ConversationPreviewLine(body: "See you\nsoon", attachment: false, from: alex)) == "See you soon")
    #expect(preview([alex], ConversationPreviewLine(body: "", attachment: true, from: alex)) == "Attachment")
    #expect(preview([alex], ConversationPreviewLine(body: "Look", attachment: true, from: nil)) == "Look")
    #expect(preview([alex, stranger], ConversationPreviewLine(body: "Hi all", attachment: false, from: alex)) == "Alex Kim: Hi all")
    #expect(preview([alex, stranger], ConversationPreviewLine(body: "", attachment: true, from: stranger)) == "+15550123: Attachment")
    #expect(preview([alex, stranger], ConversationPreviewLine(body: "Sent by me", attachment: false, from: nil)) == "Sent by me")
    #expect(preview([alex], nil) == "")
}

@Test func titlesNamePeopleOrTheirAddresses() {
    let alex = person("+15550101", "Alex Kim")
    let stranger = person("1588-0000")

    #expect(MessagesFormat.title(conversation([alex], preview: nil)) == "Alex Kim")
    #expect(MessagesFormat.title(conversation([alex, stranger], preview: nil)) == "Alex Kim, 1588-0000")
    #expect(MessagesFormat.title(conversation([], preview: nil)) == "Unknown")

    #expect(MessagesFormat.subtitle(conversation([alex], preview: nil)) == "+15550101")
    #expect(MessagesFormat.subtitle(conversation([stranger], preview: nil)) == nil)
    #expect(MessagesFormat.subtitle(conversation([alex, stranger], preview: nil)) == nil)
}

@Test func groupEventsSayWhatHappenedWhenThePhoneWroteNothing() {
    let alex = person("+15550101", "Alex Kim")
    func event(_ type: MessageGroupEvent, body: String = "", from contact: MessageContact = alex) -> String {
        MessagesFormat.groupEvent(message(1, at: 0, from: contact, body: body, groupEvent: type))
    }

    #expect(event(.joined) == "Alex Kim joined the conversation")
    #expect(event(.left, from: person("+15550123")) == "+15550123 left the conversation")
    #expect(event(.removedByLeader, from: person("")) == "Someone was removed from the conversation")
    #expect(event(.joinedSelf) == "You joined the conversation")
    #expect(event(.leftSelf) == "You left the conversation")
    #expect(event(.allParticipantsLeft) == "Everyone left the conversation")
    #expect(event(.leaderChanged) == "The group leader changed")
    #expect(event(.renamedByLeader) == "The group was renamed")
    #expect(event(.renamedByLeader, body: "Trip renamed to Busan") == "Trip renamed to Busan", "what the phone wrote")
}

@Test func messagesThePhoneHasNotDeliveredSayWhere() {
    let alex = person("+15550101", "Alex Kim")
    func status(_ mailbox: MessageMailbox, sendState: MessageSendState? = nil) -> String? {
        MessagesFormat.phoneStatus(message(1, at: 0, incoming: false, from: alex, mailbox: mailbox, sendState: sendState))
    }

    #expect(status(.outbox) == "Sending…")
    #expect(status(.queued) == "Sending…")
    #expect(status(.failed) == "Not Delivered")
    #expect(status(.draft) == "Draft")
    #expect(status(.sent) == nil)
    #expect(status(.inbox) == nil)
    #expect(status(.unknown) == nil)
    #expect(status(.sent, sendState: .failed(reason: .noService)) == nil, "a send from this Mac shows its own state")
}

@Test func unsentMessagesAreGroupedByRecipientsAndPreviewTheirState() {
    let minji = person("010-5555-6666", "김민지")
    let sameMinji = person("+82 10-5555-6666", "김민지")
    let alex = person("+15550101", "Alex Kim")

    func sent(_ id: Int64, at timestamp: Int64, body: String = "", parts: [MessagePart] = [], state: MessageSendState) -> Message {
        message(id, at: timestamp, incoming: false, from: person(""), body: body, kind: .pendingSend, parts: parts, sendState: state)
    }

    let image = MessagePart(part: .sentPart(sendId: 3, index: 0), contentType: "image/png", name: "", size: 4, state: .image, hasBytes: true)
    let groups = MessagesUnsent.groups([
        NewConversationMessage(recipients: [minji], message: sent(1, at: 100, body: "first", state: .failed(reason: .noService))),
        NewConversationMessage(recipients: [alex, minji], message: sent(2, at: 200, body: "to both", state: .sending)),
        NewConversationMessage(recipients: [sameMinji], message: sent(3, at: 300, parts: [image], state: .sent)),
    ])

    #expect(groups.map(\.id) == [["010-5555-6666"], ["+15550101", "010-5555-6666"]], "newest first, one per set of people")
    #expect(groups[0].messages.map(\.id) == [1, 3])
    #expect(MessagesFormat.title(groups[0]) == "To: 김민지")
    #expect(MessagesFormat.title(groups[1]) == "To: Alex Kim, 김민지")
    #expect(MessagesFormat.preview(groups[0]) == "Attachment", "the phone took the newest one")
    #expect(MessagesFormat.preview(groups[1]) == "Sending…")

    let failed = MessagesUnsentConversation(recipients: [minji], messages: [sent(1, at: 100, body: "first", state: .failed(reason: .noService))])
    #expect(MessagesFormat.preview(failed) == "Not Delivered · No service")

    #expect(MessagesFormat.newMessagePreview(recipients: [], text: "Hi\nthere") == "Hi there")
    #expect(MessagesFormat.newMessagePreview(recipients: [MessagesRecipient(address: "010-1", name: "Bo"), MessagesRecipient(address: "010-2", name: nil)], text: "Hi") == "To: Bo, 010-2")
}

@Test func undeliveredMessagesSayWhyWhenThePhoneDid() {
    #expect(MessagesFormat.notDelivered(.noService) == "Not Delivered · No service")
    #expect(MessagesFormat.notDelivered(.airplaneMode) == "Not Delivered · Airplane mode is on")
    #expect(MessagesFormat.notDelivered(.unknown) == "Not Delivered")
    #expect(MessagesFormat.notDelivered(.other(code: 77)) == "Not Delivered")
}

@Test func theBluetoothGateNamesWhatIsMissing() {
    #expect(MessagesFormat.bluetoothNotice(nil) == nil)
    #expect(MessagesFormat.bluetoothNotice(.connected) == nil)
    #expect(MessagesFormat.bluetoothNotice(.bluetoothOff)?.hasPrefix("Bluetooth is off") == true)
    #expect(MessagesFormat.bluetoothNotice(.phoneNotPaired)?.contains("isn't paired") == true)
    #expect(MessagesFormat.bluetoothNotice(.pairedPhoneUnavailable)?.contains("range") == true)
    #expect(MessagesFormat.bluetoothNotice(.notConnected)?.contains("isn't connected") == true)
}

@Test func attachmentsAreNamedByType() {
    func part(_ contentType: String, size: Int64) -> MessagePart {
        MessagePart(part: .mmsPart(id: 1), contentType: contentType, name: "", size: size, state: .downloadable, hasBytes: false)
    }

    let size = ByteCountFormatter.string(fromByteCount: 1_400_000, countStyle: .file)
    #expect(MessagesFormat.attachmentTitle(part("image/jpeg", size: 1_400_000)) == "Image · \(size)")
    #expect(MessagesFormat.attachmentTitle(part("video/mp4", size: 0)) == "Video")

    #expect(MessagesAttachmentKind(contentType: "IMAGE/PNG; name=a.png") == .image)
    #expect(MessagesAttachmentKind(contentType: "audio/mpeg") == .audio)
    #expect(MessagesAttachmentKind(contentType: "text/x-vcard") == .contact)
    #expect(MessagesAttachmentKind(contentType: "text/plain") == .text)
    #expect(MessagesAttachmentKind(contentType: "application/pdf") == .other)
    #expect(MessagesAttachmentKind(contentType: "") == .other)
}

@Test func suggestionsAndSimsAreLabelled() {
    func suggestion(_ kind: PhoneNumberKind, label: String = "") -> ContactSuggestion {
        ContactSuggestion(contactId: 1, name: "Alex", number: "010-1234-5678", kind: kind, label: label, hasThumbnail: false)
    }

    #expect(MessagesFormat.numberLine(suggestion(.mobile)) == "Mobile · 010-1234-5678")
    #expect(MessagesFormat.numberLine(suggestion(.custom, label: "Cabin")) == "Cabin · 010-1234-5678")
    #expect(MessagesFormat.numberLine(suggestion(.custom)) == "010-1234-5678")
    #expect(MessagesFormat.numberLine(suggestion(.unknown)) == "010-1234-5678")

    #expect(MessagesFormat.simName(SimSubscription(id: 1, simSlotIndex: 0, name: "SKT", number: "", isDefaultSms: true)) == "SKT")
    #expect(MessagesFormat.simName(SimSubscription(id: 2, simSlotIndex: 1, name: "", number: "", isDefaultSms: false)) == "SIM 2")
}

// MARK: - Rules

@Test func addressesMatchAcrossNationalAndInternationalForms() {
    #expect(MessagesAddress.same("010-1234-5678", "+82 10-1234-5678"))
    #expect(MessagesAddress.same("+821012345678", "01012345678"))
    #expect(MessagesAddress.same("(415) 555-0100", "+1 415-555-0100"))
    #expect(MessagesAddress.same("1588-0000", "15880000"))
    #expect(MessagesAddress.same("Alex@Example.com", "alex@example.com "))
    #expect(!MessagesAddress.same("010-1234-5678", "010-1234-5679"))
    #expect(!MessagesAddress.same("1234", "51234"), "short codes match only in full")
    #expect(!MessagesAddress.same("alex@example.com", "15550100"))

    #expect(MessagesAddress.sameRecipients(["010-1111-2222", "010-3333-4444"], ["+821033334444", "+821011112222"]))
    #expect(!MessagesAddress.sameRecipients(["010-1111-2222"], ["+821011112222", "+821033334444"]))
    #expect(!MessagesAddress.sameRecipients(["010-1111-2222", "010-1111-2222"], ["+821011112222", "+821033334444"]))
}

@Test func typedAddressesArePhoneNumbersOrEmail() {
    #expect(MessagesAddress.isPlausible("010-1234-5678"))
    #expect(MessagesAddress.isPlausible(" +1 (415) 555-0100 "))
    #expect(MessagesAddress.isPlausible("#123"))
    #expect(MessagesAddress.isPlausible("alex@example.com"))
    #expect(!MessagesAddress.isPlausible("12"))
    #expect(!MessagesAddress.isPlausible("Bob"))
    #expect(!MessagesAddress.isPlausible("010 Bob"))
    #expect(!MessagesAddress.isPlausible("alex@example"))
    #expect(!MessagesAddress.isPlausible("@example.com"))
}

@Test func pickedFilesKeepTheirNameAndType() throws {
    let folder = FileManager.default.temporaryDirectory.appendingPathComponent("MessagesFormatTests-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: folder) }
    let file = folder.appendingPathComponent("Beach.JPG")
    try Data([0xFF, 0xD8, 0xFF]).write(to: file)

    let attachment = try MessagesAttachment(contentsOf: file)
    #expect(attachment.name == "Beach.JPG")
    #expect(attachment.contentType == "image/jpeg")
    #expect(attachment.data == Data([0xFF, 0xD8, 0xFF]))
    #expect(throws: (any Error).self) { try MessagesAttachment(contentsOf: folder.appendingPathComponent("missing.png")) }
}

@Test func textIsCutAtCharactersWithinTheUTF16Limit() {
    #expect(MessagesTextLimit.clamp("Hello", maxUTF16: nil) == "Hello")
    #expect(MessagesTextLimit.clamp("Hello", maxUTF16: 5) == "Hello")
    #expect(MessagesTextLimit.clamp("Hello!", maxUTF16: 5) == "Hello")
    #expect(MessagesTextLimit.clamp("ab😀", maxUTF16: 3) == "ab", "an emoji is two code units and stays whole")
    #expect(MessagesTextLimit.clamp("안녕하세요", maxUTF16: 2) == "안녕")
}
