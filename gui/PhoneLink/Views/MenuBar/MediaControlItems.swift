import AppKit
import PhoneLinkKit
import SwiftUI

/// What plays on the phone and its buttons, in the menu bar extra and in the main menu's Phone menu;
/// nothing while the phone plays nothing. With two or more media apps a submenu picks the one the
/// buttons act on.
struct MediaControlItems: View {
    let media: MediaControlsModel

    var body: some View {
        if media.isShown, let session = media.selected {
            Divider()
            // A line, not a command: the title, then the artist and app as its second line.
            Label {
                Text(session.title)
                Text(session.subtitle)
            } icon: {
                Image(nsImage: MenuArtwork.thumbnail(session.albumArt))
            }
            Button {
                media.playPause()
            } label: {
                Label(session.playing ? "Pause" : "Play", systemImage: session.playing ? "pause.fill" : "play.fill")
            }
            Button {
                media.next()
            } label: {
                Label("Next Track", systemImage: "forward.fill")
            }
            Button {
                media.previous()
            } label: {
                Label("Previous Track", systemImage: "backward.fill")
            }
            if media.offersPicker {
                Picker(selection: Binding(get: { session.id }, set: { media.select($0) })) {
                    ForEach(media.sessions) { s in
                        Text(s.appName).tag(s.id)
                    }
                } label: {
                    Label("Control", systemImage: "music.note.list")
                }
            }
        }
    }
}

/// Album art at menu-item icon size; a music note when the phone sent none. A menu draws an image at
/// its own size, so the phone's art (126 px) is redrawn smaller.
enum MenuArtwork {
    static let size = NSSize(width: 32, height: 32)

    static func thumbnail(_ image: NSImage?) -> NSImage {
        guard let image else {
            let note = NSImage(systemSymbolName: "music.note", accessibilityDescription: String(localized: "Album art")) ?? NSImage()
            return note
        }

        let thumbnail = NSImage(size: size, flipped: false) { rect in
            NSBezierPath(roundedRect: rect, xRadius: 4, yRadius: 4).addClip()
            image.draw(in: rect)
            return true
        }
        thumbnail.accessibilityDescription = String(localized: "Album art")
        return thumbnail
    }
}
