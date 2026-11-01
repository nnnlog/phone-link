import SwiftUI

/// A person's picture where there is no photo: their initials on the gray gradient Apple's apps use,
/// or a person symbol for an address without a name. A photo, when given, replaces it.
struct Monogram: View {
    var name: String?
    var photo: NSImage? = nil
    var size: CGFloat = 32

    var body: some View {
        Group {
            if let photo {
                Image(nsImage: photo).resizable().scaledToFill()
            } else {
                ZStack {
                    LinearGradient(
                        colors: [Color(red: 0.65, green: 0.67, blue: 0.72), Color(red: 0.52, green: 0.54, blue: 0.59)],
                        startPoint: .top,
                        endPoint: .bottom
                    )
                    if let initials = Self.initials(of: name) {
                        Text(initials)
                            .font(.system(size: size * 0.38, weight: .medium))
                            .foregroundStyle(.white)
                            .minimumScaleFactor(0.5)
                    } else {
                        Image(systemName: "person.fill")
                            .font(.system(size: size * 0.45, weight: .medium))
                            .foregroundStyle(.white)
                    }
                }
            }
        }
        .frame(width: size, height: size)
        .clipShape(Circle())
        .accessibilityHidden(true)
    }

    /// Latin names: the first letters of the first and last words ("Sarah Lee" → "SL"). A name of one
    /// word in a script without spaces between given and family name (Korean, Chinese, Japanese): its
    /// last two characters ("김민지" → "민지"). `nil` for no name or a phone number.
    static func initials(of name: String?) -> String? {
        guard let name = name?.trimmingCharacters(in: .whitespaces), !name.isEmpty else { return nil }
        if name.unicodeScalars.allSatisfy({ CharacterSet(charactersIn: "+0123456789 -()#*").contains($0) }) {
            return nil
        }

        let words = name.split(whereSeparator: \.isWhitespace)
        if words.count > 1, let first = words.first?.first, let last = words.last?.first {
            return String(first).uppercased() + String(last).uppercased()
        }

        let word = words.first.map(String.init) ?? name
        if word.unicodeScalars.contains(where: { $0.properties.isIdeographic || (0xAC00...0xD7A3).contains($0.value) }) {
            return String(word.suffix(2))
        }
        return String(word.prefix(1)).uppercased()
    }
}
