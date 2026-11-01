import Foundation

/// Runs the Messages section's engine reads — conversations, transcripts, states, suggestions, part
/// bytes and their decoding — and hands each result back on the main actor. The model drops a result
/// that a newer read of the same kind superseded.
@MainActor
protocol MessagesReading: AnyObject {
    func read<T: Sendable>(
        _ work: @escaping @Sendable () throws -> T,
        then apply: @escaping @MainActor (Result<T, any Error>) -> Void
    )
}

/// The app's reads: off the main actor, each on its own task, so a slow store or a large part never
/// holds up the window.
final class MessagesBackgroundReads: MessagesReading {
    func read<T: Sendable>(
        _ work: @escaping @Sendable () throws -> T,
        then apply: @escaping @MainActor (Result<T, any Error>) -> Void
    ) {
        Task {
            let result = await Task.detached(priority: .userInitiated) { Result { try work() } }.value
            apply(result)
        }
    }
}

/// The section's waits.
struct MessagesTiming: Sendable {
    /// After the last keystroke in "To:", the contact search waits this long.
    var searchDelay: Duration
    /// After the phone accepted a download, its bytes are awaited this long before the download
    /// counts as failed.
    var downloadWait: Duration

    static let live = MessagesTiming(searchDelay: .milliseconds(200), downloadWait: .seconds(60))
}
