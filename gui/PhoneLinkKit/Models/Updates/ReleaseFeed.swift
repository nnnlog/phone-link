import Foundation

/// A published release of the app: a GitHub release tagged `v<version>` that carries the disk image
/// `PhoneLink-<version>.dmg`.
public struct AppRelease: Equatable, Sendable {
    public let version: AppVersion
    /// The release's notes (GitHub Markdown); empty when it has none.
    public let notes: String
    /// The release's page on GitHub.
    public let page: URL
    public let published: Date?
    public let image: DiskImage

    public struct DiskImage: Equatable, Sendable {
        public let name: String
        public let url: URL
        public let size: Int64
        /// The image's SHA-256, as GitHub computed it on upload.
        public let sha256: Data
    }
}

/// Where releases come from.
public protocol ReleaseFeed: Sendable {
    /// The repository's published releases of the app, newest first as the repository lists them.
    func releases() async throws -> [AppRelease]
}

public enum ReleaseFeedError: Error, Equatable {
    /// GitHub answered with this HTTP status.
    case http(status: Int)
    /// GitHub limits unauthenticated requests per hour and this Mac's address used them up.
    case rateLimited
    /// The answer is not the release list GitHub documents.
    case malformed(String)
}

/// The releases of a GitHub repository, from the REST API's release list (unauthenticated, so drafts
/// are not listed).
public struct GitHubReleaseFeed: ReleaseFeed {
    /// `owner/name`.
    public let repository: String
    private let session: URLSession

    /// The image a release carries, named after its version as `make-dmg.sh` names it.
    static func imageName(for version: AppVersion) -> String {
        "PhoneLink-\(version).dmg"
    }

    public init(repository: String, session: URLSession = .shared) {
        self.repository = repository
        self.session = session
    }

    public func releases() async throws -> [AppRelease] {
        guard let url = URL(string: "https://api.github.com/repos/\(repository)/releases?per_page=30") else {
            throw ReleaseFeedError.malformed("repository \(repository)")
        }

        var request = URLRequest(url: url)
        request.setValue("application/vnd.github+json", forHTTPHeaderField: "Accept")
        request.setValue("2022-11-28", forHTTPHeaderField: "X-GitHub-Api-Version")
        request.cachePolicy = .reloadIgnoringLocalCacheData

        let (data, response) = try await session.data(for: request)
        guard let http = response as? HTTPURLResponse else {
            throw ReleaseFeedError.malformed("not an HTTP response")
        }

        switch http.statusCode {
        case 200:
            return try Self.parse(data)
        case 429:
            throw ReleaseFeedError.rateLimited
        case 403 where http.value(forHTTPHeaderField: "x-ratelimit-remaining") == "0":
            throw ReleaseFeedError.rateLimited
        default:
            throw ReleaseFeedError.http(status: http.statusCode)
        }
    }

    private struct Release: Decodable {
        let tag_name: String
        let body: String?
        let draft: Bool
        let html_url: URL
        let published_at: Date?
        let assets: [Asset]
    }

    private struct Asset: Decodable {
        let name: String
        let size: Int64
        let browser_download_url: URL
        let digest: String?
    }

    /// The releases of the app in a release list. A release whose tag is not `v<version>` is not one of
    /// the app; one without its disk image yet (still uploading) is left out until it has it. An image
    /// without a SHA-256 digest is an error: the app installs nothing it cannot check.
    static func parse(_ data: Data) throws -> [AppRelease] {
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601

        let list: [Release]
        do {
            list = try decoder.decode([Release].self, from: data)
        } catch {
            throw ReleaseFeedError.malformed("release list: \(error)")
        }

        return try list.compactMap { release in
            guard !release.draft, let version = AppVersion(tag: release.tag_name) else { return nil }
            let name = imageName(for: version)
            guard let asset = release.assets.first(where: { $0.name == name }) else { return nil }
            guard let digest = asset.digest, digest.hasPrefix("sha256:"),
                  let sha256 = Data(hex: digest.dropFirst("sha256:".count)), sha256.count == 32
            else {
                throw ReleaseFeedError.malformed("\(name) of \(release.tag_name) has no SHA-256 digest")
            }

            return AppRelease(
                version: version,
                notes: release.body ?? "",
                page: release.html_url,
                published: release.published_at,
                image: .init(name: name, url: asset.browser_download_url, size: asset.size, sha256: sha256)
            )
        }
    }
}

/// Which releases the user is offered.
public enum UpdateChannel: String, Sendable, CaseIterable {
    /// Releases only.
    case stable
    /// Releases and pre-releases (`-beta.1`, `-rc.1`).
    case beta

    /// The newest release above `current` that this channel offers.
    public func newest(of releases: [AppRelease], above current: AppVersion) -> AppRelease? {
        releases
            .filter { $0.version > current && (self == .beta || !$0.version.isPreRelease) }
            .max { $0.version < $1.version }
    }
}

extension Data {
    /// Bytes from an even number of hex digits; `nil` for anything else.
    init?<S: StringProtocol>(hex: S) {
        guard hex.count.isMultiple(of: 2), hex.allSatisfy(\.isHexDigit) else { return nil }
        var bytes: [UInt8] = []
        bytes.reserveCapacity(hex.count / 2)
        var index = hex.startIndex
        while index != hex.endIndex {
            let next = hex.index(index, offsetBy: 2)
            guard let byte = UInt8(hex[index..<next], radix: 16) else { return nil }
            bytes.append(byte)
            index = next
        }
        self.init(bytes)
    }
}
