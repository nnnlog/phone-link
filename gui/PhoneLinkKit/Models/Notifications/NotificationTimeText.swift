import Foundation

/// When a phone notification was posted, in the forms Mail uses for its list and its message header.
public enum NotificationTimeText {
    /// A list row: the time today, "Yesterday", the weekday within the last week, else the date.
    public static func short(_ date: Date, now: Date = .now, calendar: Calendar = .current) -> String {
        switch daysBefore(now, date, calendar) {
        case 0:
            date.formatted(style(calendar, date: .omitted, time: .shortened))
        case 1:
            String(localized: "Yesterday")
        case 2...6:
            date.formatted(style(calendar, date: .omitted, time: .omitted).weekday(.wide))
        default:
            date.formatted(style(calendar, date: .numeric, time: .omitted))
        }
    }

    /// The detail header: the time today, else the date and the time.
    public static func full(_ date: Date, now: Date = .now, calendar: Calendar = .current) -> String {
        let dateStyle: Date.FormatStyle.DateStyle = daysBefore(now, date, calendar) == 0 ? .omitted : .abbreviated
        return date.formatted(style(calendar, date: dateStyle, time: .shortened))
    }

    /// The posting time of a notification (Unix milliseconds).
    public static func date(ofUnixMs ms: Int64) -> Date {
        Date(timeIntervalSince1970: Double(ms) / 1000)
    }

    /// Calendar days from `date` to `now` (negative when the phone's clock is ahead).
    private static func daysBefore(_ now: Date, _ date: Date, _ calendar: Calendar) -> Int {
        let days = calendar.dateComponents([.day], from: calendar.startOfDay(for: date), to: calendar.startOfDay(for: now)).day
        guard let days else { preconditionFailure("a day difference always has a day component") }
        return days
    }

    private static func style(
        _ calendar: Calendar,
        date: Date.FormatStyle.DateStyle,
        time: Date.FormatStyle.TimeStyle
    ) -> Date.FormatStyle {
        Date.FormatStyle(date: date, time: time, locale: calendar.locale ?? .current, calendar: calendar, timeZone: calendar.timeZone)
    }
}
