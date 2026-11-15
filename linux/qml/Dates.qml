pragma Singleton
import QtQuick
import PhoneLink

QtObject {
    function startOfDay(date) {
        return new Date(date.getFullYear(), date.getMonth(), date.getDate())
    }

    function daysAgo(ms) {
        const today = startOfDay(new Date())
        const day = startOfDay(new Date(ms))
        return Math.round((today - day) / 86400000)
    }

    function time(ms) {
        return new Date(ms).toLocaleTimeString(Qt.locale(), Locale.ShortFormat)
    }

    function day(ms) {
        const days = daysAgo(ms)
        if (days === 0)
            return L10n.t("Today")
        if (days === 1)
            return L10n.t("Yesterday")
        if (days < 7)
            return Qt.locale().dayName(new Date(ms).getDay(), Locale.LongFormat)
        return new Date(ms).toLocaleDateString(Qt.locale(), Locale.ShortFormat)
    }

    function listStamp(ms) {
        return daysAgo(ms) === 0 ? time(ms) : day(ms)
    }

    function detailStamp(ms) {
        return day(ms) + " " + time(ms)
    }
}
