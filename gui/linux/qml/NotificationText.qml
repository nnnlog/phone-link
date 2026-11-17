import QtQuick
import PhoneLink

// The notifications part's texts for times.
QtObject {
    // When a pause or a mute ends: "4:05 PM" today, "tomorrow at 4:05 PM" the next day (either lasts at
    // most 24 hours), else the date and the time.
    function until(ms) {
        const days = -Dates.daysAgo(ms)
        if (days === 0)
            return Dates.time(ms)
        if (days === 1)
            return L10n.f("tomorrow at %@", [Dates.time(ms)])
        return new Date(ms).toLocaleString(Qt.locale(), Locale.ShortFormat)
    }
}
