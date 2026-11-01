import Foundation
import PhoneLinkEngine

/// The kinds of field a contact card shows, each with its own generic label.
public enum ContactFieldSection: Sendable {
    case phone
    case email
    case address
    case url
    case date
}

/// The texts of a contact card.
public enum ContactsFormat {
    /// A field's label, lowercase as the Mac's Contacts writes it; a label the user typed on the phone
    /// stays as typed.
    public static func label(_ label: ContactFieldLabel, in section: ContactFieldSection) -> String {
        switch label {
        case .home: String(localized: "home")
        case .mobile: String(localized: "mobile")
        case .work: String(localized: "work")
        case .workMobile: String(localized: "work mobile")
        case .main: String(localized: "main")
        case .school: String(localized: "school")
        case .other: String(localized: "other")
        case .birthday: String(localized: "birthday")
        case .anniversary: String(localized: "anniversary")
        case let .custom(label): label
        case .generic:
            switch section {
            case .phone: String(localized: "phone")
            case .email: String(localized: "email")
            case .address: String(localized: "address")
            case .url: String(localized: "homepage")
            case .date: String(localized: "date")
            }
        }
    }

    /// The line under a card's name: the job title and the company, as far as they are set.
    public static func subtitle(_ card: ContactCard) -> String? {
        let parts = [card.jobTitle, card.company == card.name ? "" : card.company].filter { !$0.isEmpty }
        return parts.isEmpty ? nil : parts.joined(separator: " · ")
    }
}
