import Foundation
import PhoneLinkEngine

/// How a call shows in the history: its label and symbol, and whether it is drawn as a missed call.
public struct CallKindStyle: Equatable, Sendable {
    /// "Incoming", "Outgoing", "Missed", "Declined"; empty for a call of no known type.
    public let label: String
    /// SF Symbol; `nil` for a call of no known type.
    public let symbol: String?
    /// Missed and declined calls are drawn in red.
    public let missed: Bool
}

/// A day bucket of the history, newest first. "Last 30 Days" covers 7 to 30 days back.
public enum CallsBucket: Int, CaseIterable, Comparable, Sendable {
    case today
    case yesterday
    case thisWeek
    case lastThirtyDays
    case older

    public static func < (lhs: CallsBucket, rhs: CallsBucket) -> Bool {
        lhs.rawValue < rhs.rawValue
    }

    public var title: String {
        switch self {
        case .today: String(localized: "Today")
        case .yesterday: String(localized: "Yesterday")
        case .thisWeek: String(localized: "This Week")
        case .lastThirtyDays: String(localized: "Last 30 Days")
        case .older: String(localized: "Older")
        }
    }
}

/// One section of the history: a day bucket and its calls, newest first.
public struct CallsSection: Identifiable, Equatable, Sendable {
    public let bucket: CallsBucket
    public let calls: [CallHistoryItem]

    public var id: CallsBucket { bucket }
    public var title: String { bucket.title }
}

/// The Calls section's texts and groupings, on an injectable clock and calendar (the calendar's
/// locale and time zone format the text).
public struct CallsFormat: Sendable {
    public var now: @Sendable () -> Date
    public var calendar: Calendar

    public init(now: @escaping @Sendable () -> Date = { Date() }, calendar: Calendar = .autoupdatingCurrent) {
        self.now = now
        self.calendar = calendar
    }

    /// The person: the contact's name, else the number as the phone stores it, else "Private Number".
    public static func name(_ call: CallHistoryItem) -> String {
        if let name = call.name, !name.isEmpty { return name }
        return call.number.isEmpty ? String(localized: "Private Number") : call.number
    }

    /// Voicemail, blocked and answered-elsewhere calls show as incoming, as Phone Link does.
    public static func style(_ kind: CallKind) -> CallKindStyle {
        switch kind {
        case .incoming, .voicemail, .blocked, .answeredExternally:
            CallKindStyle(label: String(localized: "Incoming"), symbol: "phone.arrow.down.left", missed: false)
        case .outgoing:
            CallKindStyle(label: String(localized: "Outgoing"), symbol: "phone.arrow.up.right", missed: false)
        case .missed:
            CallKindStyle(label: String(localized: "Missed"), symbol: "phone.arrow.down.left", missed: true)
        case .rejected:
            CallKindStyle(label: String(localized: "Declined"), symbol: "phone.down", missed: true)
        case .unknown:
            CallKindStyle(label: "", symbol: nil, missed: false)
        }
    }

    /// The bucket of a call that started at `ms`; a start after today (the phone's clock is ahead)
    /// counts as today.
    public func bucket(ms: Int64) -> CallsBucket {
        let today = calendar.startOfDay(for: now())
        let day = calendar.startOfDay(for: Self.date(ms: ms))
        guard let days = calendar.dateComponents([.day], from: day, to: today).day else {
            preconditionFailure("a day difference always has a day component")
        }

        switch days {
        case ...0: return .today
        case 1: return .yesterday
        case 2...6: return .thisWeek
        case 7...30: return .lastThirtyDays
        default: return .older
        }
    }

    /// `calls` (newest first) in their day buckets; empty buckets are left out.
    public func sections(_ calls: [CallHistoryItem]) -> [CallsSection] {
        let grouped = Dictionary(grouping: calls) { bucket(ms: $0.startMs) }
        return grouped.keys.sorted().map { bucket in
            guard let calls = grouped[bucket] else { preconditionFailure("a key of the dictionary has a value") }
            return CallsSection(bucket: bucket, calls: calls.sorted { ($0.startMs, $0.callId) > ($1.startMs, $1.callId) })
        }
    }

    /// A row's time, by its bucket: the time today and yesterday, the weekday within the week, the
    /// month and day within 30 days, else the date.
    public func rowTime(ms: Int64) -> String {
        let date = Self.date(ms: ms)
        switch bucket(ms: ms) {
        case .today, .yesterday: return date.formatted(style(time: .shortened))
        case .thisWeek: return date.formatted(style().weekday(.wide))
        case .lastThirtyDays: return date.formatted(style().month(.defaultDigits).day(.defaultDigits))
        case .older: return date.formatted(style(date: .numeric))
        }
    }

    /// The detail's time: the date and the time.
    public func fullTime(ms: Int64) -> String {
        Self.date(ms: ms).formatted(style(date: .abbreviated, time: .shortened))
    }

    /// How long a call lasted, as "1:05" or "1:02:05"; `nil` for a call of no length (one that was
    /// not answered).
    public static func duration(seconds: Int64) -> String? {
        guard seconds > 0 else { return nil }
        let (hours, minutes, secs) = (seconds / 3600, seconds / 60 % 60, seconds % 60)
        if hours > 0 {
            return String(format: "%lld:%02lld:%02lld", hours, minutes, secs)
        }
        return String(format: "%lld:%02lld", minutes, secs)
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
