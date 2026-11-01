import PhoneLinkEngine
import UniformTypeIdentifiers

/// The messaging MIME-type lookups from the system's type declarations.
public final class UTTypeFileTypes: MessagingFileTypes {
    /// Office and word-processing formats; RTF conforms to text, so it is a text file.
    private static let documents: [UTType] = [.pdf, .presentation, .spreadsheet] + [
        "org.openxmlformats.wordprocessingml.document",
        "com.microsoft.word.doc",
        "org.oasis-open.opendocument.text",
    ].compactMap { UTType($0) }

    public init() {}

    /// A MIME type no declared type has (or one with parameters) is a dynamic type that conforms to
    /// none of these, so it is other.
    public func family(contentType: String) -> FileFamily {
        guard let type = UTType(mimeType: contentType) else { return .other }
        return Self.family(of: type)
    }

    public func fileExtension(contentType: String) -> String? {
        UTType(mimeType: contentType)?.preferredFilenameExtension.map { "." + $0 }
    }

    /// Conformance tested in the trait's order: vCard, image, audio, movie, text, documents.
    static func family(of type: UTType) -> FileFamily {
        if type.conforms(to: .vCard) { return .vCard }
        if type.conforms(to: .image) { return .image }
        if type.conforms(to: .audio) { return .audio }
        if type.conforms(to: .movie) { return .video }
        if type.conforms(to: .text) { return .text }
        if documents.contains(where: { type.conforms(to: $0) }) { return .document }
        return .other
    }
}
