import AppKit

/// macOS's standard About panel (the app's icon, name, version and copyright) with credits: links to
/// the app's GitHub repository (`PhoneLinkUpdateRepository` in the Info.plist), then the notice that the
/// app is not Microsoft's. Without a repository the credits hold only the notice.
@MainActor
enum AboutPanel {
    static func show() {
        // An app without a Dock icon is not active while its menu is open; without this the panel
        // opens behind the app in front.
        NSApp.activate()
        NSApp.orderFrontStandardAboutPanel(options: [.credits: credits()])
    }

    private static func credits() -> NSAttributedString {
        let attributes: [NSAttributedString.Key: Any] = [
            .font: NSFont.systemFont(ofSize: NSFont.smallSystemFontSize),
            .foregroundColor: NSColor.labelColor,
        ]

        let credits = NSMutableAttributedString()
        let repository = (Bundle.main.object(forInfoDictionaryKey: "PhoneLinkUpdateRepository") as? String) ?? ""
        if !repository.isEmpty {
            guard let home = URL(string: "https://github.com/\(repository)") else {
                preconditionFailure("PhoneLinkUpdateRepository makes a GitHub URL: \(repository)")
            }

            let links = [
                ("GitHub", home),
                (String(localized: "Releases"), home.appending(path: "releases")),
                (String(localized: "Report an Issue"), home.appending(path: "issues")),
                // `HEAD` opens the file on the repository's default branch.
                (String(localized: "License"), home.appending(path: "blob/HEAD/LICENSE")),
            ]

            for (index, (title, url)) in links.enumerated() {
                if index > 0 {
                    credits.append(NSAttributedString(string: " · ", attributes: attributes))
                }
                credits.append(NSAttributedString(string: title, attributes: attributes.merging([.link: url]) { $1 }))
            }
            credits.append(NSAttributedString(string: "\n\n", attributes: attributes))
        }

        let notice = String(
            localized: "Phone Link for macOS is an independent project and is neither affiliated with, nor authorized, sponsored, or approved by, Microsoft Corporation."
        )
        credits.append(NSAttributedString(string: notice, attributes: attributes))

        let centered = NSMutableParagraphStyle()
        centered.alignment = .center
        credits.addAttribute(.paragraphStyle, value: centered, range: NSRange(location: 0, length: credits.length))
        return credits
    }
}
