import AppKit
import Testing
@testable import PhoneLinkKit

struct MessagesReturnKeyTests {
    @Test func returnSendsAndAModifiedReturnStartsANewLine() {
        let cases: [(NSEvent.ModifierFlags, MessagesReturnKey)] = [
            ([], .send),
            (.command, .send),
            // What a Return key event carries besides its modifiers.
            ([.numericPad, .function], .send),
            (.capsLock, .send),
            (.shift, .newLine),
            (.option, .newLine),
            (.control, .newLine),
            ([.option, .shift], .newLine),
            ([.command, .shift], .newLine),
        ]

        for (modifiers, expected) in cases {
            #expect(MessagesReturnKey(modifiers: modifiers) == expected, "modifiers \(modifiers.rawValue)")
        }
    }
}
