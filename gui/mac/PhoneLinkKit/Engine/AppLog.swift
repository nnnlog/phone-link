import OSLog
import PhoneLinkEngine

/// The app's logger: each line goes to the unified log (Console) and to the app's log file, which
/// Settings › Advanced saves or clears.
///
/// Interpolations follow the unified log's privacy rules: a number or a Bool is public, anything else
/// is private unless marked `privacy: .public`. The log file writes a private value as a
/// `{{REDACTED:private|<tag>}}` token, equal for equal values, and scrubs a public one of the paths,
/// URLs, and identifiers it quotes; the unified log shows `<private>`. `notice` is for what is
/// unexpected but handled, and the file writes it as `WARN`.
public struct AppLog: Sendable {
    private let category: String
    private let logger: Logger

    public init(category: String) {
        self.category = category
        logger = Logger(subsystem: PhoneLinkIdentity.bundleID, category: category)
    }

    public func debug(_ message: LogMessage) { write(.debug, message) }
    public func info(_ message: LogMessage) { write(.info, message) }
    public func notice(_ message: LogMessage) { write(.notice, message) }
    public func error(_ message: LogMessage) { write(.error, message) }
    public func fault(_ message: LogMessage) { write(.fault, message) }

    private func write(_ level: LogLevel, _ message: LogMessage) {
        let shown = message.segments.map { $0.isPublic ? $0.text : "<private>" }.joined()
        switch level {
        case .debug: logger.debug("\(shown, privacy: .public)")
        case .info: logger.info("\(shown, privacy: .public)")
        case .notice: logger.notice("\(shown, privacy: .public)")
        case .error: logger.error("\(shown, privacy: .public)")
        case .fault: logger.fault("\(shown, privacy: .public)")
        }

        logMessage(level: level, category: category, segments: message.segments)
    }
}

/// Whether an interpolated value is written as it is.
public enum LogPrivacy: Sendable {
    case `public`
    case `private`
}

/// A log line as literal text and interpolated values, each marked public or private.
public struct LogMessage: ExpressibleByStringInterpolation, Sendable {
    let segments: [LogSegment]

    public init(stringLiteral value: String) {
        segments = [LogSegment(text: value, isPublic: true)]
    }

    public init(stringInterpolation: StringInterpolation) {
        segments = stringInterpolation.segments
    }

    static func describe(_ error: any Error) -> String {
        switch error as? HotspotError {
        case .NotVisible?: "NotVisible"
        case let .JoinFailed(_, _, reason)?: "JoinFailed(reason: \(String(reflecting: reason)))"
        default: String(describing: error)
        }
    }

    public struct StringInterpolation: StringInterpolationProtocol {
        var segments: [LogSegment] = []

        public init(literalCapacity: Int, interpolationCount: Int) {
            segments.reserveCapacity(2 * interpolationCount + 1)
        }

        public mutating func appendLiteral(_ literal: String) {
            append(literal, .public)
        }

        public mutating func appendInterpolation<T>(_ value: T, privacy: LogPrivacy = .private) {
            append(String(describing: value), privacy)
        }

        public mutating func appendInterpolation<T: BinaryInteger>(_ value: T, privacy: LogPrivacy = .public) {
            append(String(describing: value), privacy)
        }

        public mutating func appendInterpolation<T: BinaryFloatingPoint>(_ value: T, privacy: LogPrivacy = .public) {
            append(String(describing: value), privacy)
        }

        public mutating func appendInterpolation(_ value: Bool, privacy: LogPrivacy = .public) {
            append(String(describing: value), privacy)
        }

        /// An error: its description, public (the log file scrubs it of paths, URLs, and
        /// identifiers), without the credentials a hotspot error carries for the user's manual join.
        public mutating func appendInterpolation(error: any Error) {
            append(LogMessage.describe(error), .public)
        }

        private mutating func append(_ text: String, _ privacy: LogPrivacy) {
            guard !text.isEmpty else { return }
            let isPublic = privacy == .public

            // Adjacent public text is one segment, so the file's line is not split needlessly.
            if isPublic, let last = segments.last, last.isPublic {
                segments[segments.count - 1] = LogSegment(text: last.text + text, isPublic: true)
            } else {
                segments.append(LogSegment(text: text, isPublic: isPublic))
            }
        }
    }
}
