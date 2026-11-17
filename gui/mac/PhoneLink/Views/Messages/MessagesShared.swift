import AppKit
import PhoneLinkEngine
import PhoneLinkKit
import SwiftUI
import UniformTypeIdentifiers

/// A conversation's picture: the person's contact photo or monogram, or a group's monogram.
struct MessagesAvatar: View {
    let recipients: [MessageContact]
    let size: CGFloat

    init(conversation: Conversation, size: CGFloat) {
        self.init(recipients: conversation.recipients, size: size)
    }

    init(recipients: [MessageContact], size: CGFloat) {
        self.recipients = recipients
        self.size = size
    }

    var body: some View {
        if recipients.count > 1 {
            MessagesGroupMonogram(size: size)
        } else {
            MessagesContactAvatar(contact: recipients.first, size: size)
        }
    }
}

/// One person's picture: their contact's photo when the phone has one, else their monogram.
struct MessagesContactAvatar: View {
    let contact: MessageContact?
    let size: CGFloat

    var body: some View {
        let name = contact.map(MessagesFormat.name)
        if let id = contact?.photoContactId {
            MessagesAsyncImage(picture: .contact(id), maxPixels: Int(size * 2)) { photo in
                Monogram(name: name, photo: photo, size: size)
            }
        } else {
            Monogram(name: name, size: size)
        }
    }
}

/// A group conversation's picture: the shared monogram's gray gradient (its colors are private to
/// `Monogram`) with two people on it.
struct MessagesGroupMonogram: View {
    let size: CGFloat

    var body: some View {
        Circle()
            .fill(LinearGradient(
                colors: [Color(red: 0.65, green: 0.67, blue: 0.72), Color(red: 0.52, green: 0.54, blue: 0.59)],
                startPoint: .top,
                endPoint: .bottom
            ))
            .overlay {
                Image(systemName: "person.2.fill")
                    .font(.system(size: size * 0.4, weight: .medium))
                    .foregroundStyle(.white)
            }
            .frame(width: size, height: size)
            .accessibilityHidden(true)
    }
}

/// Shows a picture the model reads and decodes off the main actor: `content` gets `nil` until it is
/// there (and when there is none).
struct MessagesAsyncImage<Content: View>: View {
    let picture: MessagesPicture
    let maxPixels: Int
    @ViewBuilder let content: (NSImage?) -> Content
    @Environment(MessagesModel.self) private var model
    @State private var image: NSImage?

    var body: some View {
        content(image)
            .onChange(of: picture) {
                // A reused view shows nothing of the picture it showed before.
                image = nil
            }
            .task(id: picture) {
                image = await model.image(of: picture, maxPixels: maxPixels)
            }
    }
}

/// An image dropped on a conversation: a file, with its name and type, or image data.
struct MessagesDroppedImage: Transferable {
    let attachment: MessagesAttachment

    static var transferRepresentation: some TransferRepresentation {
        FileRepresentation(importedContentType: .image) { received in
            MessagesDroppedImage(attachment: try MessagesAttachment(contentsOf: received.file))
        }
        DataRepresentation(importedContentType: .image) { data in
            MessagesDroppedImage(attachment: MessagesAttachment(name: "", contentType: nil, data: data))
        }
    }
}
