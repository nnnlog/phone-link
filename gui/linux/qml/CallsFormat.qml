import QtQuick
import PhoneLink

// The Calls section's texts and groupings: who a call is with, how its kind shows, its day group, its
// times, how long it lasted or has run, and the search.
QtObject {
    // The person: the contact's name, else the number as the phone stores it, else "Private Number".
    function name(call) {
        if (call.name)
            return call.name
        return call.number.length > 0 ? call.number : L10n.t("Private Number")
    }

    // A current call's person: the contact's name, else the number, else "Unknown".
    function activeName(call) {
        if (call.name)
            return call.name
        return call.number.length > 0 ? call.number : L10n.t("Unknown")
    }

    // The number under a current call's contact name; empty when the name is the number.
    function activeNumber(call) {
        return call.name && call.number.length > 0 ? call.number : ""
    }

    // How a kind of call shows: its label, its icon, and whether it is drawn as missed. Voicemail,
    // blocked and answered-elsewhere calls show as incoming; a call of no known kind has no label.
    function style(kind) {
        switch (kind) {
        case "incoming":
        case "voicemail":
        case "blocked":
        case "answeredExternally":
            return { label: L10n.t("Incoming"), icon: "phoneIn", missed: false }
        case "outgoing":
            return { label: L10n.t("Outgoing"), icon: "phoneOut", missed: false }
        case "missed":
            return { label: L10n.t("Missed"), icon: "phoneIn", missed: true }
        case "rejected":
            return { label: L10n.t("Declined"), icon: "phoneDown", missed: true }
        default:
            return { label: "", icon: "", missed: false }
        }
    }

    // The day group of a call that started at `ms`: 0 today (also a start after today, when the
    // phone's clock is ahead), 1 yesterday, 2 this week, 3 the last 30 days, 4 older.
    function bucket(ms) {
        const days = Dates.daysAgo(ms)
        if (days <= 0)
            return 0
        if (days === 1)
            return 1
        if (days <= 6)
            return 2
        if (days <= 30)
            return 3
        return 4
    }

    function bucketTitle(bucket) {
        switch (bucket) {
        case 0: return L10n.t("Today")
        case 1: return L10n.t("Yesterday")
        case 2: return L10n.t("This Week")
        case 3: return L10n.t("Last 30 Days")
        default: return L10n.t("Older")
        }
    }

    // A row's time, by its group: the time today and yesterday, the weekday within the week, the
    // month and day within 30 days, else the date.
    function rowTime(ms) {
        const date = new Date(ms)
        switch (bucket(ms)) {
        case 0:
        case 1:
            return Dates.time(ms)
        case 2:
            return Qt.locale().dayName(date.getDay(), Locale.LongFormat)
        case 3:
            return date.toLocaleDateString(Qt.locale(), Qt.locale().dateFormat(Locale.ShortFormat)
                                           .replace(/[^A-Za-z]*y+[^A-Za-z]*/, ""))
        default:
            return date.toLocaleDateString(Qt.locale(), Locale.ShortFormat)
        }
    }

    // The detail's time: the date with the month's short name, and the time.
    function fullTime(ms) {
        const format = Qt.locale().dateFormat(Locale.LongFormat).replace(/d{3,4}[,.]?\s*/, "")
                       .replace(/[,\s]+$/, "").trim().replace("MMMM", "MMM")
        return new Date(ms).toLocaleDateString(Qt.locale(), format) + " " + Dates.time(ms)
    }

    function clock(seconds) {
        const hours = Math.floor(seconds / 3600)
        const minutes = Math.floor(seconds / 60) % 60
        const rest = seconds % 60
        const two = number => (number < 10 ? "0" : "") + number
        return hours > 0 ? hours + ":" + two(minutes) + ":" + two(rest) : minutes + ":" + two(rest)
    }

    // How long a call lasted, as "1:05" or "1:02:05"; empty for a call of no length (not answered).
    function duration(seconds) {
        return seconds > 0 ? clock(seconds) : ""
    }

    // How long a current call has run since `sinceMs`, at `nowMs`.
    function elapsed(sinceMs, nowMs) {
        return clock(Math.max(0, Math.floor((nowMs - sinceMs) / 1000)))
    }

    // Whether `call` matches the search: its shown name holds the query (ignoring case), or its
    // number holds the query's digits.
    function matches(call, query) {
        const trimmed = query.trim()
        if (trimmed.length === 0)
            return true
        if (name(call).toLocaleLowerCase().includes(trimmed.toLocaleLowerCase()))
            return true
        const digits = trimmed.replace(/[^0-9]/g, "")
        return digits.length > 0 && call.number.replace(/[^0-9]/g, "").includes(digits)
    }

    // The list's rows for the calls that match `query`: each day group's header, then its calls,
    // newest first.
    function rows(calls, query) {
        const result = []
        let group = -1
        calls.filter(call => matches(call, query))
            .map(call => ({ call: call, bucket: bucket(call.startMs) }))
            .sort((a, b) => a.bucket - b.bucket || b.call.startMs - a.call.startMs || b.call.id - a.call.id)
            .forEach(entry => {
                if (entry.bucket !== group) {
                    group = entry.bucket
                    result.push({ header: bucketTitle(group), key: "header:" + group })
                }
                result.push({ call: entry.call, key: "call:" + entry.call.id })
            })
        return result
    }
}
