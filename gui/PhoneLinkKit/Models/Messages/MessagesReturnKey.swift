import AppKit

/// What Return does in the field a message or a reply is written in: it sends, as Command-Return
/// does, and with Shift, Option or Control it starts a new line.
public enum MessagesReturnKey: Equatable, Sendable {
    case send
    case newLine

    /// The meaning of Return pressed with `modifiers`.
    public init(modifiers: NSEvent.ModifierFlags) {
        self = modifiers.isDisjoint(with: [.shift, .option, .control]) ? .send : .newLine
    }
}
