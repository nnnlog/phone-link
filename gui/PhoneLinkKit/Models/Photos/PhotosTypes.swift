import AppKit
import Observation
import PhoneLinkEngine
import UniformTypeIdentifiers

/// One gallery item and the picture its cell shows. Each cell observes its own item, so a thumbnail
/// that arrives redraws that cell alone.
@MainActor
@Observable
public final class PhotosGridItem: Identifiable {
    public nonisolated let id: Int64
    public internal(set) var photo: Photo
    /// The thumbnail, or the full image scaled down when the item has no thumbnail; `nil` until loaded.
    public internal(set) var thumbnail: NSImage?
    /// A file action, a save of several photos that has yet to reach this one, or the delete
    /// pre-check is waiting on the phone for this item.
    public var isBusy: Bool { pendingWork > 0 }

    var pendingWork = 0
    /// Bumped when the item's image changed on the phone, so a load still decoding the old one is dropped.
    @ObservationIgnored var imageGeneration = 0
    /// The generation `thumbnail` was decoded from.
    @ObservationIgnored var thumbnailGeneration: Int?

    init(photo: Photo) {
        id = photo.id
        self.photo = photo
    }

    /// The item shows an image: a thumbnail or the full image is cached (what makes it actionable).
    var showsImage: Bool { photo.hasThumbnail || photo.hasFull }
}

/// What the Photos section shows, in the order the states take precedence.
public enum PhotosContent: Equatable, Sendable {
    /// Nothing yet: the engine has not started.
    case nothing
    /// The phone offers no photos to this Mac.
    case unavailable
    /// "Show photos on this Mac" is off.
    case turnedOff
    /// The phone doesn't allow Phone Link to see its photos.
    case needsPhoneAccess
    case empty
    case gallery
}

/// An alert of the Photos section.
public enum PhotosPrompt: Equatable, Sendable {
    /// Deleting on the phone can't be undone, so the user confirms it. The alert names the photo, or
    /// counts the photos: it shows once the phone answered the pre-check, when the selection may have
    /// moved on. `name` is the one photo's name, empty for several.
    case confirmDelete(ids: [Int64], name: String)
    /// The phone hasn't allowed deleting photos; the alert offers to ask it.
    case allowDeleting
    /// An action the user started failed.
    case failure(title: String, message: String)

    public var title: String {
        switch self {
        case let .confirmDelete(ids, _) where ids.count > 1: String(localized: "Delete \(ids.count) Photos from Your Phone?")
        case .confirmDelete: String(localized: "Delete This Photo from Your Phone?")
        case .allowDeleting: String(localized: "Allow Deleting Photos on Your Phone?")
        case let .failure(title, _): title
        }
    }

    public var message: String {
        switch self {
        case let .confirmDelete(ids, _) where ids.count > 1:
            String(localized: "These \(ids.count) photos will be deleted from your phone. This can't be undone.")
        case let .confirmDelete(_, name) where name.isEmpty: String(localized: "This can't be undone.")
        case let .confirmDelete(_, name): String(localized: "“\(name)” will be deleted from your phone. This can't be undone.")
        case .allowDeleting: String(localized: "Your phone needs to allow Phone Link to delete photos.")
        case let .failure(_, message): message
        }
    }
}

/// What Quick Look shows for the selected photo.
public struct PhotosPreview: Equatable, Sendable {
    public let id: Int64
    public let title: String
    /// The full image; until it arrives — or when its bytes are no photo — the thumbnail, and `nil`
    /// while there is no photo to show. Named after its bytes' image type.
    public let file: URL?
    public let isFullImage: Bool
    /// The version of the photo the file holds: a photo edited on the phone keeps its file name, and
    /// the new version still makes a new preview that Quick Look reloads.
    let modifiedMs: Int64
}

/// A file action's full image on its way: fetched when not cached (at most 30 s); the actions that
/// hand a file on write it to the run's folder under the photo's save-as name once it is here.
public struct PhotosPendingFile: Sendable {
    public let action: PhotoAction
    /// The save-as name (stem and extension).
    public let fileName: String
    /// The type the file's extension names.
    public let contentType: UTType
    let id: Int64
    let name: PhotoFileName
    /// The gallery's departures and the selection when the action started: a result that arrives
    /// after the user moved on is dropped by the actions that pop up or launch something.
    let departures: Int
    let selection: Set<Int64>
    let task: Task<Result<Data, PhotosActionFailure>, Never>
}

/// The full image of a photo, written for a file action.
public struct PhotosActionFile: Sendable {
    public let url: URL
    public let data: Data
}

/// Why a file action got no image, in the words the alert shows.
struct PhotosActionFailure: Error, Sendable {
    let message: String
}
