import CoreGraphics
import CoreImage
import CoreImage.CIFilterBuiltins
import Foundation

/// What the post-notification sheet shows during notification setup: the link that opens the
/// setting on the phone — a QR code of
/// `https://aka.ms/notification?src=1&traceId=<a fresh GUID>`, and the short form
/// `aka.ms/notification` to type. Nothing is sent to the phone.
public struct PostNotificationHelp: Identifiable, Equatable, Sendable {
    /// The short form of the link, for the user to type on the phone.
    public static let shortLink = "aka.ms/notification"

    public let id: UUID
    /// The link the code holds; its `traceId` is a fresh lower-case GUID for each sheet.
    public let link: URL

    public init(id: UUID = UUID()) {
        guard let link = URL(string: "https://aka.ms/notification?src=1&traceId=\(id.uuidString.lowercased())") else {
            preconditionFailure("the post-notification link is a URL")
        }
        self.id = id
        self.link = link
    }

    /// The link as a QR code (error correction M), one module per `scale` pixels, dark on light.
    public func qrCode(scale: CGFloat = 8) -> CGImage {
        let filter = CIFilter.qrCodeGenerator()
        filter.message = Data(link.absoluteString.utf8)
        filter.correctionLevel = "M"

        guard let output = filter.outputImage else {
            preconditionFailure("Core Image encodes a short URL as a QR code")
        }

        let scaled = output.transformed(by: CGAffineTransform(scaleX: scale, y: scale))
        guard let image = CIContext().createCGImage(scaled, from: scaled.extent) else {
            preconditionFailure("Core Image renders the QR code")
        }
        return image
    }
}
