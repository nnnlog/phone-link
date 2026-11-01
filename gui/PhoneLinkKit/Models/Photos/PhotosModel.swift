import AppKit
import Observation
import os
import PhoneLinkEngine
import UniformTypeIdentifiers

private let log = AppLog(category: "photos")

/// The Photos section: the phone's recent gallery and the thumbnails its cells show, the selection
/// (one photo or several, as in the Finder), the fetch triggers the grid and Quick Look report to the
/// engine, the file actions — each first gets the full image, waiting at most 30 s — delete with its
/// confirmation and permission prompts, and the photo permission requests to the phone. Full images
/// are read and files written off the main actor.
@MainActor
@Observable
public final class PhotosModel {
    /// The feature's state; `nil` until the engine has started.
    public private(set) var state: PhotosState?
    /// The gallery in the engine's display order.
    public private(set) var items: [PhotosGridItem] = []
    /// The selected items, always ones the gallery lists: the subject of the toolbar, the Edit menu
    /// and the file actions.
    public private(set) var selection: Set<Int64> = []
    /// The photo the arrow keys move from and Quick Look shows: the last one clicked or moved to,
    /// always one of the selection.
    public private(set) var focus: Int64?
    /// The alert to show.
    public var prompt: PhotosPrompt?
    /// What Quick Look shows while it is open.
    public private(set) var preview: PhotosPreview?
    /// A permission request to the phone is under way; its button waits for it.
    public var requestPending: Bool { requestsUnderWay > 0 }
    private var requestsUnderWay = 0

    private let api: any PhotosApiProtocol
    private let files: PhotosTemporaryFiles
    private let pasteboard: NSPasteboard
    /// Opens a file with an app, the default one for `nil` (injected so tests never launch an app).
    private let openFile: @MainActor (URL, URL?) async throws -> Void
    /// How many decoded thumbnails stay cached once their cells scrolled away.
    private let thumbnailLimit: Int
    /// Quick Look's file requests: a queued one that is no longer the newest skips its work.
    private let previewRequests = PhotosRequestCounter()

    @ObservationIgnored private var itemsByID: [Int64: PhotosGridItem] = [:]
    /// The items whose cells are on screen.
    @ObservationIgnored private var visible: Set<Int64> = []
    /// Items with a decoded thumbnail, least recently shown first.
    @ObservationIgnored private var thumbnailOrder: [Int64] = []
    /// The image generation each item's thumbnail is being decoded from.
    @ObservationIgnored private var loading: [Int64: Int] = [:]
    /// The pending report of the cells on screen after a layout pass.
    @ObservationIgnored private(set) var layoutReport: Task<Void, Never>?
    /// The latest write of a Quick Look file.
    @ObservationIgnored private(set) var previewLoad: Task<Void, Never>?
    /// The file Quick Look wants of the focused item: shown, or being written.
    @ObservationIgnored private var previewSource: PreviewSource?
    /// Where a Shift-click range starts, and the selection that range adds to.
    @ObservationIgnored private var anchor: Int64?
    @ObservationIgnored private var rangeBase: Set<Int64> = []
    @ObservationIgnored private var viewerOpen = false
    @ObservationIgnored private var galleryWasLeft = false
    /// How many times the gallery was left: a file action compares it with the count at its start.
    @ObservationIgnored private var galleryDepartures = 0
    /// A thumbnail read, or a report to the engine, failed since the last full reload (each is logged
    /// once: every cell repeats it).
    @ObservationIgnored private var thumbnailFailureLogged = false
    @ObservationIgnored private var callFailureLogged = false

    /// Cells are at most 180 pt wide; twice that covers a Retina display.
    private static let thumbnailPixelSize = 360

    public convenience init(api: any PhotosApiProtocol, hub: EngineEventHub) {
        self.init(
            api: api,
            hub: hub,
            temporaryDirectory: FileManager.default.temporaryDirectory,
            pasteboard: .general,
            openFile: PhotosModel.openInWorkspace
        )
    }

    init(
        api: any PhotosApiProtocol,
        hub: EngineEventHub,
        temporaryDirectory: URL,
        pasteboard: NSPasteboard,
        openFile: @escaping @MainActor (URL, URL?) async throws -> Void,
        thumbnailLimit: Int = 240
    ) {
        self.api = api
        files = PhotosTemporaryFiles(base: temporaryDirectory)
        self.pasteboard = pasteboard
        self.openFile = openFile
        self.thumbnailLimit = thumbnailLimit

        hub.onEvent { [weak self] event in
            self?.handle(event)
        }
        hub.onStart { [weak self] in
            self?.reload()
        }

        _ = NotificationCenter.default.addObserver(
            forName: NSApplication.willTerminateNotification,
            object: nil,
            queue: .main
        ) { [files] _ in
            files.removeAll()
        }
    }

    /// What the section shows.
    public var content: PhotosContent {
        guard let state else { return .nothing }
        if !state.enabled { return .unavailable }
        if !state.consent { return .turnedOff }
        if !state.photosPermission { return .needsPhoneAccess }
        return items.isEmpty ? .empty : .gallery
    }

    /// The selected items' ids in gallery order.
    public var selectedIDs: [Int64] {
        items.compactMap { selection.contains($0.id) ? $0.id : nil }
    }

    /// What a context menu's actions apply to: the selection when the clicked item is part of it,
    /// otherwise the clicked item alone.
    public func targets(clicked id: Int64) -> [Int64] {
        selection.contains(id) ? selectedIDs : [id]
    }

    /// The items of `ids` that can be deleted now.
    public func deletable(_ ids: [Int64]) -> [Int64] {
        ids.filter { itemsByID[$0].map(canDelete) ?? false }
    }

    // MARK: - Engine events

    private func handle(_ event: EngineEvent) {
        switch event {
        case let .photos(change):
            handle(change)
        case .resync(feature: .photos):
            reload()
        default:
            break
        }
    }

    private func handle(_ change: PhotosChange) {
        switch change {
        case .galleryChanged:
            reloadGallery(imagesChanged: false)
        case .stateChanged:
            reloadState()
        case let .fullImageStored(id):
            // The gallery's flags follow with `GalleryChanged`; Quick Look switches to the image now.
            if viewerOpen, focus == id {
                updatePreview(failureAlert: false, fullImageStored: true)
            }
        case let .deleteFinished(result):
            if result.failed {
                prompt = .failure(title: String(localized: "Couldn't Delete the Photo"), message: String(localized: "The photo is still on your phone."))
            }
        case let .openImage(id):
            openFromNotification(id)
        }
    }

    /// Re-reads everything: the engine started, or this feature's events were lost.
    private func reload() {
        thumbnailFailureLogged = false
        callFailureLogged = false
        reloadState()
        reloadGallery(imagesChanged: true)
    }

    private func reloadState() {
        do {
            state = try api.state()
        } catch {
            record(error, "photos state not read")
        }
    }

    /// Re-reads the gallery: keeps the items still listed (with their thumbnails unless their image
    /// changed, or every one when `imagesChanged`) and drops the others with their thumbnails and
    /// files; a photo whose full image changed loses its files too.
    private func reloadGallery(imagesChanged: Bool) {
        let photos: [Photo]
        do {
            photos = try api.gallery()
        } catch {
            record(error, "photo gallery not read")
            return
        }

        var next: [PhotosGridItem] = []
        var byID: [Int64: PhotosGridItem] = [:]
        next.reserveCapacity(photos.count)
        for photo in photos {
            let item: PhotosGridItem
            if let existing = itemsByID[photo.id] {
                item = existing
                let previous = existing.photo
                if previous != photo {
                    existing.photo = photo
                }
                if imagesChanged || Self.imageChanged(from: previous, to: photo) {
                    invalidateImage(of: existing)
                }
                if Self.fullImageChanged(from: previous, to: photo) {
                    files.removeFiles(of: photo.id)
                }
            } else {
                item = PhotosGridItem(photo: photo)
            }

            guard byID.updateValue(item, forKey: photo.id) == nil else {
                preconditionFailure("the engine listed photo \(photo.id) twice")
            }
            next.append(item)
        }

        for id in itemsByID.keys where byID[id] == nil {
            loading[id] = nil
            files.removeFiles(of: id)
        }

        thumbnailOrder.removeAll { byID[$0] == nil }
        itemsByID = byID
        if next.map(\.id) != items.map(\.id) {
            items = next
        }

        let kept = selection.filter { byID[$0] != nil }
        if kept != selection {
            selection = kept
        }
        rangeBase = rangeBase.filter { byID[$0] != nil }
        if let anchor, byID[anchor] == nil {
            self.anchor = nil
        }
        if let focus, byID[focus] == nil {
            setFocus(nil)
        }

        for id in visible {
            if let item = byID[id] {
                loadThumbnail(of: item)
            }
        }

        if imagesChanged {
            previewSource = nil
        }
        updatePreview(failureAlert: false)
    }

    /// The bytes behind the item's picture may differ: it was modified, or it gained or lost a
    /// thumbnail, or — without one — the full image its cell scales down.
    private static func imageChanged(from old: Photo, to new: Photo) -> Bool {
        old.modifiedMs != new.modifiedMs
            || old.hasThumbnail != new.hasThumbnail
            || (!new.hasThumbnail && old.hasFull != new.hasFull)
    }

    /// The photo's full image may differ from the files written of it: it was modified, or the engine
    /// dropped its copy (cache maintenance, or the phone reported new content).
    private static func fullImageChanged(from old: Photo, to new: Photo) -> Bool {
        old.modifiedMs != new.modifiedMs || (old.hasFull && !new.hasFull)
    }

    /// Marks the item's thumbnail out of date. An item on screen keeps showing it until the new one
    /// is decoded; one off screen drops it and loads again when it appears.
    private func invalidateImage(of item: PhotosGridItem) {
        item.imageGeneration += 1
        if !visible.contains(item.id) || !item.showsImage {
            dropThumbnail(of: item)
        }
    }

    // MARK: - Grid

    /// A cell appeared: the engine fetches its thumbnail when missing, and the cell gets its picture.
    public func itemAppeared(_ id: Int64) {
        visible.insert(id)
        call("visible photo not passed on") { try api.itemVisible(id: id) }
        if let item = itemsByID[id] {
            if item.thumbnail != nil {
                markRecentlyShown(id)
            }
            loadThumbnail(of: item)
        }
        scheduleLayoutReport()
    }

    /// A cell disappeared: the engine cancels the fetches the grid queued for it. Once the gallery was
    /// left, its thumbnail goes too.
    public func itemDisappeared(_ id: Int64) {
        visible.remove(id)
        call("hidden photo not passed on") { try api.itemHidden(id: id) }
        if galleryWasLeft, let item = itemsByID[id] {
            dropThumbnail(of: item)
        }
        scheduleLayoutReport()
    }

    /// The gallery is on screen again after it was left (the window reopened): its cells are
    /// reported again, since the fetches queued for them were cancelled.
    public func galleryEntered() {
        guard galleryWasLeft else { return }
        galleryWasLeft = false
        for item in items where visible.contains(item.id) {
            call("visible photo not passed on") { try api.itemVisible(id: item.id) }
        }
        scheduleLayoutReport()
    }

    /// The gallery left the screen (another section, or the window closed): every queued fetch is
    /// cancelled, Quick Look closes, and the thumbnails of cells off screen are let go — the app may
    /// run for weeks with its window closed.
    public func galleryLeft() {
        galleryWasLeft = true
        galleryDepartures += 1
        call("gallery leave not passed on") { try api.galleryLeft() }
        closeViewer()
        for item in items where item.thumbnail != nil && !visible.contains(item.id) {
            dropThumbnail(of: item)
        }
    }

    /// An item's context menu opened: the engine fetches its full image ahead of an action.
    public func contextMenuOpened(_ id: Int64) {
        guard lists(id, "context menu") else { return }
        call("context menu not passed on") { try api.contextMenuOpened(id: id) }
    }

    /// A click: the item alone is selected (`nil`: nothing is), and a Shift-click range starts there.
    public func select(_ id: Int64?) {
        if let id, !lists(id, "selection") {
            return
        }

        let selected: Set<Int64> = id.map { [$0] } ?? []
        if selection != selected {
            selection = selected
        }

        anchor = id
        rangeBase = []
        setFocus(id)
    }

    /// A Command-click: the item joins the selection, or leaves it. Either way a Shift-click range
    /// starts there and adds to the selection as it now stands.
    public func toggleSelection(_ id: Int64) {
        guard lists(id, "selection") else { return }
        anchor = id
        if selection.contains(id) {
            selection.remove(id)
            if focus == id {
                setFocus(nil)
            }
        } else {
            selection.insert(id)
            setFocus(id)
        }
        rangeBase = selection
    }

    /// A Shift-click: the items from the anchor to this one, in gallery order, join the selection as
    /// it stood when the anchor was set, so a second Shift-click replaces the first one's range rather
    /// than adding to it. Without an anchor it is a click.
    public func extendSelection(to id: Int64) {
        guard lists(id, "selection") else { return }
        guard let anchor,
              let from = items.firstIndex(where: { $0.id == anchor }),
              let to = items.firstIndex(where: { $0.id == id })
        else {
            select(id)
            return
        }

        let selected = rangeBase.union(items[min(from, to)...max(from, to)].map(\.id))
        if selection != selected {
            selection = selected
        }
        setFocus(id)
    }

    /// Select All: every item the gallery lists. The focus and the anchor stay where they are.
    public func selectAll() {
        let all = Set(items.map(\.id))
        if selection != all {
            selection = all
        }
    }

    /// Moves the focus `offset` items through the gallery (the arrow keys), selecting that item alone;
    /// the first item when nothing has the focus. A move past either end keeps the selection.
    public func moveSelection(by offset: Int) {
        guard let first = items.first else { return }
        guard let focus, let index = items.firstIndex(where: { $0.id == focus }) else {
            select(first.id)
            return
        }
        if items.indices.contains(index + offset) {
            select(items[index + offset].id)
        }
    }

    /// The focus moves to `id`; Quick Look, when open, follows it or closes.
    private func setFocus(_ id: Int64?) {
        guard focus != id else { return }
        focus = id
        guard viewerOpen else { return }
        if let id {
            showInViewer(id)
        } else {
            closeViewer()
        }
    }

    /// The gallery lists `id`. An id the view still holds for an item that left the gallery (the
    /// photo under the pointer was deleted) is logged and ignored.
    private func lists(_ id: Int64, _ subject: StaticString) -> Bool {
        guard itemsByID[id] == nil else { return true }
        log.debug("\(subject.description, privacy: .public) of photo \(id, privacy: .private) ignored; the gallery no longer lists it")
        return false
    }

    /// The grid's layout pass ends after the cells of one update appeared and disappeared, so the
    /// visible set is reported once, after them.
    private func scheduleLayoutReport() {
        guard layoutReport == nil else { return }
        layoutReport = Task { [weak self] in
            self?.reportLayout()
        }
    }

    private func reportLayout() {
        layoutReport = nil
        let shown = items.map(\.id).filter(visible.contains)
        guard !shown.isEmpty else { return }
        call("grid layout not passed on") { try api.gridLayout(visible: shown) }
    }

    // MARK: - Thumbnails

    /// Decodes the item's picture off the main thread unless it is current or under way: its
    /// thumbnail, or without one its full image scaled down.
    private func loadThumbnail(of item: PhotosGridItem) {
        guard item.showsImage else {
            dropThumbnail(of: item)
            return
        }

        let generation = item.imageGeneration
        guard item.thumbnailGeneration != generation, loading[item.id] != generation else { return }
        loading[item.id] = generation

        let id = item.id
        let fromFullImage = !item.photo.hasThumbnail
        let maxPixelSize = Self.thumbnailPixelSize
        Task { [api] in
            let decoded = await Task.detached(priority: .userInitiated) {
                Result { () throws -> CGImage? in
                    let data = try fromFullImage ? api.fullImage(id: id) : api.thumbnail(id: id)
                    return data.flatMap { PhotosImageDecoder.thumbnail(of: $0, maxPixelSize: maxPixelSize) }
                }
            }.value
            self.thumbnailDecoded(decoded, of: item, generation: generation)
        }
    }

    private func thumbnailDecoded(_ decoded: Result<CGImage?, any Error>, of item: PhotosGridItem, generation: Int) {
        if loading[item.id] == generation {
            loading[item.id] = nil
        }

        guard itemsByID[item.id] === item, item.imageGeneration == generation else { return }
        switch decoded {
        case let .success(image?):
            // A cell that left the screen after the gallery was left keeps no picture.
            guard !galleryWasLeft || visible.contains(item.id) else { return }
            item.thumbnail = NSImage(cgImage: image, size: .zero)
            item.thumbnailGeneration = generation
            markRecentlyShown(item.id)
        case .success(nil):
            log.debug("photo \(item.id, privacy: .private) shows no thumbnail; no bytes, or bytes ImageIO does not read")
            item.thumbnail = nil
            item.thumbnailGeneration = generation
        case let .failure(error):
            if let apiError = error as? FeatureApiError, case .NotStarted = apiError { return }
            if !thumbnailFailureLogged {
                thumbnailFailureLogged = true
                log.error("photo thumbnail not read: \(error: error); later failures are not logged until the next start")
            }
        }
    }

    private func markRecentlyShown(_ id: Int64) {
        thumbnailOrder.removeAll { $0 == id }
        thumbnailOrder.append(id)
        while thumbnailOrder.count > thumbnailLimit,
              let victim = thumbnailOrder.first(where: { !visible.contains($0) }),
              let item = itemsByID[victim]
        {
            dropThumbnail(of: item)
        }
    }

    private func dropThumbnail(of item: PhotosGridItem) {
        item.thumbnail = nil
        item.thumbnailGeneration = nil
        thumbnailOrder.removeAll { $0 == item.id }
    }

    // MARK: - Quick Look

    /// Opens Quick Look on an item (double-click, Space) when it shows an image; the engine fetches
    /// its full image, and Quick Look shows the thumbnail until it arrives.
    public func openViewer(_ id: Int64) {
        guard lists(id, "quick look") else { return }
        let opens: Bool
        do {
            opens = try api.openItem(id: id)
        } catch {
            record(error, "photo not opened in quick look")
            return
        }
        guard opens else { return }

        if !selection.contains(id) {
            selection = [id]
            anchor = id
            rangeBase = []
        }

        focus = id
        viewerOpen = true
        showInViewer(id)
    }

    /// Space: Quick Look opens on the focused item, or closes.
    public func toggleViewer() {
        if viewerOpen {
            closeViewer()
        } else if let focus {
            openViewer(focus)
        }
    }

    /// Quick Look closed (or is to close): the file being written for it is not wanted any more, and
    /// the files it showed go.
    public func closeViewer() {
        guard viewerOpen else { return }
        viewerOpen = false
        previewRequests.outdateAll()
        previewSource = nil
        if let shown = preview {
            files.removeViewerFiles(of: shown.id)
            preview = nil
        }

        call("viewer close not passed on") { try api.viewerClosed() }
    }

    /// The viewer moved to `id` (the user opened it or moved the focus): the engine prefetches
    /// it and its neighbours and stamps it seen.
    private func showInViewer(_ id: Int64) {
        call("viewer selection not passed on") { try api.viewerSelection(id: id) }
        call("photo seen not passed on") { try api.viewerItemShown(id: id) }
        updatePreview(failureAlert: true)
    }

    /// Brings Quick Look's file in line with the focus: the best image cached of the photo's
    /// current version — the full image, else the thumbnail, else none (the panel's loading state).
    /// Quick Look gets only photos, in files named after their bytes (`PhotosTemporaryFiles
    /// .viewerFileName`): full bytes that are none leave it on the thumbnail. The image is read and its
    /// file written off the main actor; until then Quick Look shows what it showed. `fullImageStored`
    /// reads the full image before the gallery's flags say it is cached.
    private func updatePreview(failureAlert: Bool, fullImageStored: Bool = false) {
        guard viewerOpen, let id = focus else { return }
        guard let item = itemsByID[id] else {
            preconditionFailure("Quick Look shows photo \(id), which the gallery does not list")
        }

        let kind: PreviewSource.Kind =
            fullImageStored || item.photo.hasFull ? .fullImage : item.photo.hasThumbnail ? .thumbnail : .nothing
        let source = PreviewSource(id: id, kind: kind, modifiedMs: item.photo.modifiedMs)
        guard source != previewSource else { return }
        previewSource = source

        let request = previewRequests.start()
        let name: PhotoFileName
        let title: String
        do {
            name = try api.saveAsName(id: id)
            title = try PhotosTemporaryFiles.fileName(name)
        } catch {
            previewFailed(error, alert: failureAlert)
            return
        }
        guard kind != .nothing else {
            showPreview(PhotosPreview(id: id, title: title, file: nil, isFullImage: false, modifiedMs: source.modifiedMs))
            return
        }

        let job: @Sendable () throws -> PreviewWrite = { [api, files, previewRequests] in
            guard previewRequests.isNewest(request) else { return .skipped }
            let thumbnail = { try api.thumbnail(id: id).flatMap { try files.writeThumbnail($0, id: id, name: name) } }
            if kind == .fullImage, let data = try api.fullImage(id: id) {
                if let file = try files.writeViewerImage(data, id: id, name: name) {
                    return .written(file: file, isFullImage: true)
                }
                return .fullImageNoPhoto(thumbnail: try thumbnail())
            }
            // The thumbnail, also when the engine no longer has the full image.
            return .written(file: try thumbnail(), isFullImage: false)
        }

        previewLoad = Task { [files] in
            let written: Result<PreviewWrite, any Error>
            do {
                written = .success(try await files.perform(job))
            } catch {
                written = .failure(error)
            }

            previewWritten(written, of: source, title: title, request: request, failureAlert: failureAlert)
        }
    }

    private func previewWritten(
        _ written: Result<PreviewWrite, any Error>,
        of source: PreviewSource,
        title: String,
        request: Int,
        failureAlert: Bool
    ) {
        guard previewRequests.isNewest(request) else {
            // Quick Look closed, moved to another photo, or wants a better file of this one.
            if case let .success(write) = written, write.wroteFiles, !(viewerOpen && focus == source.id) {
                files.removeViewerFiles(of: source.id)
            }
            return
        }

        switch written {
        case .success(.skipped):
            preconditionFailure("the newest Quick Look file request skipped its work")
        case let .success(.fullImageNoPhoto(thumbnail)):
            // This version's full image stays out of Quick Look: no new file is wanted until it changes.
            log.notice("full image of photo \(source.id, privacy: .private) not shown in quick look; ImageIO does not read it, the thumbnail shows")
            showPreview(PhotosPreview(
                id: source.id,
                title: title,
                file: thumbnail,
                isFullImage: false,
                modifiedMs: source.modifiedMs
            ))
        case let .success(.written(file, isFullImage)):
            if source.kind == .fullImage && !isFullImage {
                // The engine no longer has the full image: Quick Look waits for it again.
                previewSource = PreviewSource(
                    id: source.id,
                    kind: file == nil ? .nothing : .thumbnail,
                    modifiedMs: source.modifiedMs
                )
            }

            showPreview(PhotosPreview(
                id: source.id,
                title: title,
                file: file,
                isFullImage: isFullImage,
                modifiedMs: source.modifiedMs
            ))
        case let .failure(error):
            previewFailed(error, alert: failureAlert)
        }
    }

    /// Quick Look shows `next`; the files it showed of another photo go.
    private func showPreview(_ next: PhotosPreview) {
        if let shown = preview, shown.id != next.id {
            files.removeViewerFiles(of: shown.id)
        }
        if preview != next {
            preview = next
        }
    }

    /// Quick Look has no file for the focused item: when the user just opened it or moved the focus,
    /// it closes with the alert; a later update is only logged and retried with the next change.
    private func previewFailed(_ error: any Error, alert: Bool) {
        previewSource = nil
        if alert {
            closeViewer()
            log.error("photo not shown in quick look: \(error: error)")
            prompt = .failure(title: String(localized: "Couldn't Show the Photo"), message: UserText.describe(error))
        } else {
            if let apiError = error as? FeatureApiError, case .NotStarted = apiError { return }
            log.notice("quick look not updated: \(error: error); it keeps what it shows")
        }
    }

    // MARK: - File actions

    /// Starts getting the full image for a file action — fetched when not cached, waited for at most
    /// 30 s — so a picker can show while it arrives. `nil` (after the alert) when the item has no
    /// usable file name, and for Open, Open With and drag-out when that name is not a photo's or
    /// video's: those hand the file to the app its extension names, and the extension comes from
    /// the phone (a drag-out shows no alert, the drag carries nothing).
    public func startFile(_ id: Int64, for action: PhotoAction) -> PhotosPendingFile? {
        do {
            return try pendingFile(id, for: action)
        } catch let error as PhotosNotPhotoOrVideo {
            if action != .dragOut {
                prompt = .failure(title: Self.failureTitle(action), message: UserText.describe(error))
            }
            return nil
        } catch {
            log.error("\(Self.failureSubject(action).description, privacy: .public): \(error: error)")
            prompt = .failure(title: Self.failureTitle(action), message: UserText.describe(error))
            return nil
        }
    }

    /// `startFile` without the alert: throws why the action gets no file (`PhotosNotPhotoOrVideo`
    /// for a name refused to the app, logged here).
    private func pendingFile(_ id: Int64, for action: PhotoAction) throws -> PhotosPendingFile {
        let name = try api.saveAsName(id: id)
        let fileName = try PhotosTemporaryFiles.fileName(name)
        guard let contentType = UTType(filenameExtension: String(name.extension.dropFirst())) else {
            throw PhotosUnusableFileName(name: fileName)
        }
        if Self.handsFileToApp(action), !Self.isPhotoOrVideo(contentType) {
            log.notice("photo \(id, privacy: .private) not handed to an app; its name has the type \(contentType.identifier, privacy: .private)")
            throw PhotosNotPhotoOrVideo(name: fileName)
        }

        let item = itemsByID[id]
        item?.pendingWork += 1
        let task = Task { [api] () -> Result<Data, PhotosActionFailure> in
            defer { item?.pendingWork -= 1 }
            do {
                guard let data = try await api.fullImageForAction(id: id, action: action) else {
                    log.error("\(Self.failureSubject(action).description, privacy: .public); its full image did not arrive within 30 s")
                    return .failure(PhotosActionFailure(message: UserText.failure(.timedOut)!))
                }
                return .success(data)
            } catch {
                // Logged here, where it happens: the alert may be dropped when the user moved on.
                log.error("\(Self.failureSubject(action).description, privacy: .public): \(error: error)")
                return .failure(PhotosActionFailure(message: UserText.describe(error)))
            }
        }

        return PhotosPendingFile(
            action: action,
            fileName: fileName,
            contentType: contentType,
            id: id,
            name: name,
            departures: galleryDepartures,
            selection: selection,
            task: task
        )
    }

    /// The type Open With lists apps for: the photo's type, named by its save-as extension — what
    /// Launch Services opens it as — when it is a photo or video type; `nil` otherwise, and Open With
    /// then lists no app.
    public func openWithContentType(of id: Int64) -> UTType? {
        let type: UTType?
        do {
            type = UTType(filenameExtension: String(try api.saveAsName(id: id).extension.dropFirst()))
        } catch {
            record(error, "photo file name not read")
            return nil
        }
        return type.flatMap { Self.isPhotoOrVideo($0) ? $0 : nil }
    }

    /// The file of a started action, or `nil` after the alert that says why there is none.
    public func file(from pending: PhotosPendingFile) async -> PhotosActionFile? {
        guard let data = image(await pending.task.value, of: pending) else { return nil }
        return await write(data, of: pending)
    }

    /// The file of a started Share, or `nil`: after the alert that says why there is none, or —
    /// logged, without one — when the user moved on while it was on its way (left the gallery, or
    /// changed the selection), as the picker would point at what is no longer in front of them.
    public func fileToShare(from pending: PhotosPendingFile) async -> PhotosActionFile? {
        let outcome = await pending.task.value
        guard !movedOn(since: pending, orSelectionMoved: true),
              let data = image(outcome, of: pending),
              let file = await write(data, of: pending),
              !movedOn(since: pending, orSelectionMoved: true)
        else {
            return nil
        }
        return file
    }

    /// Copy: the pasteboard gets the file of a temporary copy and the image data — unless the user
    /// copied something else while the image was on its way.
    @discardableResult
    public func copy(_ id: Int64) -> Task<Void, Never>? {
        copy([id])
    }

    /// Copy of several photos: the pasteboard gets one entry per photo, in the order given, once
    /// every image is here; nothing when one of them fails (after its alert) or the user copied
    /// something else meanwhile.
    @discardableResult
    public func copy(_ ids: [Int64]) -> Task<Void, Never>? {
        let changeCount = pasteboard.changeCount
        var pendings: [PhotosPendingFile] = []
        for id in ids {
            guard let pending = startFile(id, for: .copy) else { return nil }
            pendings.append(pending)
        }
        guard !pendings.isEmpty else { return nil }

        return Task {
            var entries: [NSPasteboardItem] = []
            for pending in pendings {
                guard let data = image(await pending.task.value, of: pending),
                      !pasteboardChanged(since: changeCount, copying: pending.id),
                      let file = await write(data, of: pending),
                      !pasteboardChanged(since: changeCount, copying: pending.id)
                else {
                    return
                }

                let entry = NSPasteboardItem()
                entry.setString(file.url.absoluteString, forType: .fileURL)
                if pending.contentType.conforms(to: .image) {
                    entry.setData(file.data, forType: NSPasteboard.PasteboardType(pending.contentType.identifier))
                }
                entries.append(entry)
            }

            pasteboard.clearContents()
            if !pasteboard.writeObjects(entries) {
                log.error("photo not copied; the clipboard refused it")
                prompt = .failure(title: Self.failureTitle(.copy), message: String(localized: "The clipboard didn't accept the photo."))
            }
        }
    }

    /// Open (with `app`, or the default app for the photo's type).
    @discardableResult
    public func open(_ id: Int64, with app: URL? = nil) -> Task<Void, Never>? {
        guard let pending = startFile(id, for: app == nil ? .open : .openWith) else { return nil }
        return open(pending, with: app)
    }

    /// Opens a started action's file once it is here (Open With › Other… picks the app meanwhile);
    /// dropped, logged, when the gallery was left meanwhile — an app launching late surprises.
    @discardableResult
    public func open(_ pending: PhotosPendingFile, with app: URL?) -> Task<Void, Never> {
        Task {
            let outcome = await pending.task.value
            guard !movedOn(since: pending, orSelectionMoved: false),
                  let data = image(outcome, of: pending),
                  let file = await write(data, of: pending),
                  !movedOn(since: pending, orSelectionMoved: false)
            else {
                return
            }

            do {
                try await openFile(file.url, app)
            } catch {
                log.error("photo not opened: \(error: error)")
                prompt = .failure(title: Self.failureTitle(pending.action), message: UserText.describe(error))
            }
        }
    }

    /// Save As: writes a started action's image where the save panel chose, once it is here.
    @discardableResult
    public func save(_ pending: PhotosPendingFile, to destination: URL) -> Task<Void, Never> {
        Task {
            guard let data = image(await pending.task.value, of: pending) else { return }
            do {
                try await Task.detached(priority: .userInitiated) {
                    try data.write(to: destination, options: .atomic)
                }.value
            } catch {
                log.error("photo not saved: \(error: error)")
                prompt = .failure(title: Self.failureTitle(.saveAs), message: UserText.describe(error))
            }
        }
    }

    /// Save N Photos…: writes each photo's full image into `folder` under its save-as name, with
    /// " 2", " 3"… added when the folder already holds that name. One photo at a time, as each fetch
    /// waits at most 30 s for the phone: the photos still to come show as busy until their turn ends.
    /// Every photo is tried; one alert at the end counts those that weren't saved.
    @discardableResult
    public func save(_ ids: [Int64], toFolder folder: URL) -> Task<Void, Never> {
        let queued = ids.map { itemsByID[$0] }
        for item in queued {
            item?.pendingWork += 1
        }

        return Task {
            var failures: [String] = []
            for (id, item) in zip(ids, queued) {
                defer { item?.pendingWork -= 1 }
                do {
                    let pending = try pendingFile(id, for: .saveAs)
                    let data = try await pending.task.value.get()
                    let name = pending.name
                    _ = try await Task.detached(priority: .userInitiated) {
                        try PhotosModel.writeNumbered(data, name: name, into: folder)
                    }.value
                } catch let failure as PhotosActionFailure {
                    // Logged where the fetch failed.
                    failures.append(failure.message)
                } catch {
                    log.error("photo not saved: \(error: error)")
                    failures.append(UserText.describe(error))
                }
            }

            guard let reason = failures.first else { return }
            let title = failures.count == 1 ? Self.failureTitle(.saveAs) : String(localized: "Couldn't Save \(failures.count) Photos")
            let saved = ids.count - failures.count
            let message = saved == 0 ? reason : String(localized: "\(saved) of \(ids.count) photos were saved. \(reason)")
            prompt = .failure(title: title, message: message)
        }
    }

    /// Writes `data` into `folder` under the first of the photo's numbered names the folder doesn't
    /// hold yet; never over an existing file.
    nonisolated static func writeNumbered(_ data: Data, name: PhotoFileName, into folder: URL) throws -> URL {
        var number = 1
        while true {
            let destination = folder.appendingPathComponent(try numberedFileName(name, number: number))
            if !FileManager.default.fileExists(atPath: destination.path) {
                try data.write(to: destination, options: .withoutOverwriting)
                return destination
            }
            number += 1
        }
    }

    /// The save-as name for `number` 1, and for a later number the name with " <number>" before
    /// the extension, as the Finder numbers copies ("IMG_0001 2.jpg").
    nonisolated static func numberedFileName(_ name: PhotoFileName, number: Int) throws -> String {
        guard number > 1 else { return try PhotosTemporaryFiles.fileName(name) }
        return try PhotosTemporaryFiles.fileName(PhotoFileName(stem: "\(name.stem) \(number)", extension: name.extension))
    }

    /// A new-photo notification was clicked and the engine stored the full image: it opens in the
    /// default viewer when its name is a photo's or video's. Not started from the window, so a failure
    /// is only logged.
    private func openFromNotification(_ id: Int64) {
        let name: PhotoFileName
        do {
            name = try api.saveAsName(id: id)
        } catch {
            record(error, "clicked new photo not opened")
            return
        }

        guard let type = UTType(filenameExtension: String(name.extension.dropFirst())), Self.isPhotoOrVideo(type)
        else {
            log.notice("clicked new photo \(id, privacy: .private) not opened; its name is no photo's or video's")
            return
        }

        Task { [api, files] in
            do {
                let written = try await files.perform { () -> URL? in
                    guard let data = try api.fullImage(id: id) else { return nil }
                    return try files.writeFullImage(data, id: id, name: name)
                }
                guard let url = written else {
                    log.notice("clicked new photo \(id, privacy: .private) not opened; the engine has no full image of it")
                    return
                }

                try await openFile(url, nil)
            } catch {
                record(error, "clicked new photo not opened")
            }
        }
    }

    /// The image of a started action, or `nil` after the alert that says why there is none.
    private func image(_ outcome: Result<Data, PhotosActionFailure>, of pending: PhotosPendingFile) -> Data? {
        switch outcome {
        case let .success(data):
            return data
        case let .failure(failure):
            prompt = .failure(title: Self.failureTitle(pending.action), message: failure.message)
            return nil
        }
    }

    /// Writes a started action's file off the main actor; `nil` after the alert when it can't be.
    private func write(_ data: Data, of pending: PhotosPendingFile) async -> PhotosActionFile? {
        let (files, id, name) = (files, pending.id, pending.name)
        do {
            let url = try await files.perform { try files.writeFullImage(data, id: id, name: name) }
            return PhotosActionFile(url: url, data: data)
        } catch {
            log.error("\(Self.failureSubject(pending.action).description, privacy: .public): \(error: error)")
            prompt = .failure(title: Self.failureTitle(pending.action), message: UserText.describe(error))
            return nil
        }
    }

    /// The user moved on since the action started: the gallery was left, or — with
    /// `orSelectionMoved` — the selection changed.
    private func movedOn(since pending: PhotosPendingFile, orSelectionMoved: Bool) -> Bool {
        let movedOn = galleryDepartures != pending.departures || (orSelectionMoved && selection != pending.selection)
        if movedOn {
            log.debug("\(String(describing: pending.action), privacy: .public) of photo \(pending.id, privacy: .private) dropped; the user moved on")
        }
        return movedOn
    }

    private func pasteboardChanged(since changeCount: Int, copying id: Int64) -> Bool {
        guard pasteboard.changeCount != changeCount else { return false }
        log.debug("copy of photo \(id, privacy: .private) dropped; the clipboard changed meanwhile")
        return true
    }

    /// Open, Open With and drag-out hand the file to the app its type names.
    private static func handsFileToApp(_ action: PhotoAction) -> Bool {
        switch action {
        case .open, .openWith, .dragOut: true
        case .copy, .saveAs, .share: false
        }
    }

    /// Only a photo or video is handed to an app: the extension comes from the phone, and a name such
    /// as `.terminal` or `.webloc` would run its handler.
    nonisolated static func isPhotoOrVideo(_ type: UTType) -> Bool {
        isPhoto(type) || type.conforms(to: .audiovisualContent)
    }

    /// An image type other than SVG, which is left out though it is an image type: it can carry
    /// scripts, and its default app may be a browser.
    nonisolated static func isPhoto(_ type: UTType) -> Bool {
        type.conforms(to: .image) && !type.conforms(to: .svg)
    }

    private static func failureSubject(_ action: PhotoAction) -> StaticString {
        switch action {
        case .copy, .dragOut: "photo not copied"
        case .saveAs: "photo not saved"
        case .share: "photo not shared"
        case .open, .openWith: "photo not opened"
        }
    }

    private static func failureTitle(_ action: PhotoAction) -> String {
        switch action {
        case .copy, .dragOut: String(localized: "Couldn't Copy the Photo")
        case .saveAs: String(localized: "Couldn't Save the Photo")
        case .share: String(localized: "Couldn't Share the Photo")
        case .open, .openWith: String(localized: "Couldn't Open the Photo")
        }
    }

    /// Opens with Launch Services: the default app, or the chosen one.
    static func openInWorkspace(_ file: URL, with app: URL?) async throws {
        guard let app else {
            guard NSWorkspace.shared.open(file) else { throw PhotosNoAppToOpen() }
            return
        }
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, any Error>) in
            NSWorkspace.shared.open([file], withApplicationAt: app, configuration: NSWorkspace.OpenConfiguration()) {
                _, error in
                if let error {
                    continuation.resume(throwing: error)
                } else {
                    continuation.resume()
                }
            }
        }
    }

    // MARK: - Delete

    /// The item's Delete is offered: the phone offers deleting and has it enabled, the item shows an
    /// image, and no delete of it is under way.
    public func canDelete(_ item: PhotosGridItem) -> Bool {
        state?.deleteAvailable == true && item.showsImage && item.photo.deleteState == .none
    }

    /// Delete…: the pre-check first (it may ask the phone silently for the permission), then the
    /// confirmation naming the photo — none when it left the gallery meanwhile — or the "allow
    /// deleting" prompt.
    @discardableResult
    public func delete(_ id: Int64) -> Task<Void, Never>? {
        delete([id])
    }

    /// Delete of several photos: those of `ids` that can be deleted are pre-checked one by one, then
    /// one confirmation counts those still in the gallery; the first that lacks the permission shows
    /// the "allow deleting" prompt instead. `nil` when none can be deleted.
    @discardableResult
    public func delete(_ ids: [Int64]) -> Task<Void, Never>? {
        let targets = ids.compactMap { itemsByID[$0] }.filter(canDelete)
        guard !targets.isEmpty else { return nil }
        for item in targets {
            item.pendingWork += 1
        }

        return Task {
            defer {
                for item in targets {
                    item.pendingWork -= 1
                }
            }

            do {
                for item in targets {
                    switch try await api.prepareDelete(id: item.id) {
                    case .confirm:
                        continue
                    case .missingPermission:
                        prompt = .allowDeleting
                        return
                    }
                }
            } catch {
                log.error("photo delete not prepared: \(error: error)")
                prompt = .failure(title: Self.deleteFailureTitle(count: targets.count), message: UserText.describe(error))
                return
            }

            let remaining = targets.filter { itemsByID[$0.id] === $0 }
            if remaining.count < targets.count {
                log.debug("delete of \(targets.count - remaining.count, privacy: .public) photos dropped; they left the gallery before the confirmation")
            }
            guard let first = remaining.first else { return }
            prompt = .confirmDelete(ids: remaining.map(\.id), name: remaining.count == 1 ? first.photo.name : "")
        }
    }

    /// The user confirmed: the phone deletes the photos, one after another. A failure it reports
    /// arrives as `DeleteFinished`; a call that fails stops the rest.
    @discardableResult
    public func confirmDelete(_ ids: [Int64]) -> Task<Void, Never> {
        Task {
            do {
                for id in ids {
                    _ = try await api.delete(id: id)
                }
            } catch {
                log.error("photo not deleted: \(error: error)")
                prompt = .failure(title: Self.deleteFailureTitle(count: ids.count), message: UserText.describe(error))
            }
        }
    }

    private static func deleteFailureTitle(count: Int) -> String {
        count == 1 ? String(localized: "Couldn't Delete the Photo") : String(localized: "Couldn't Delete the Photos")
    }

    /// "Allow on Phone": asks the phone to allow deleting; the user deletes again afterwards.
    @discardableResult
    public func allowDeletingOnPhone() -> Task<Void, Never> {
        sendRequest(failureTitle: String(localized: "Couldn't Allow Deleting Photos"), subject: "photo delete permission not granted", limitedAccessAnswers: false) { [api] in
            try await api.requestDeletePermission()
        }
    }

    // MARK: - Permissions and settings

    /// "Send Request to Phone" (photo access is off on the phone).
    @discardableResult
    public func requestPhotoAccess() -> Task<Void, Never> {
        sendRequest(failureTitle: String(localized: "Couldn't Get Photo Access"), subject: "photo access not granted", limitedAccessAnswers: true) { [api] in
            try await api.requestPhotoAccess()
        }
    }

    /// "Allow Full Access" (the phone shares only some photos).
    @discardableResult
    public func requestFullAccess() -> Task<Void, Never> {
        sendRequest(failureTitle: String(localized: "Couldn't Get Full Photo Access"), subject: "full photo access not granted", limitedAccessAnswers: true) { [api] in
            try await api.requestFullAccess()
        }
    }

    /// "Not Now" on the full-access reminder (it returns with the next connection).
    public func dismissFullAccessReminder() {
        do {
            try api.dismissFullAccessReminder()
        } catch {
            log.error("full-access reminder not hidden: \(error: error)")
            prompt = .failure(title: String(localized: "Couldn't Hide the Reminder"), message: UserText.describe(error))
        }
    }

    /// "Turn On": shows photos on this Mac again.
    public func turnOn() {
        do {
            try api.setConsent(on: true)
        } catch {
            log.error("photos not turned on: \(error: error)")
            prompt = .failure(title: String(localized: "Couldn't Turn On Photos"), message: UserText.describe(error))
        }
    }

    /// Sends a permission request; the state change it causes arrives as an event. NeedsPermission
    /// answers a photo-access request with limited access, so with `limitedAccessAnswers` it is no
    /// failure.
    private func sendRequest(
        failureTitle: String,
        subject: StaticString,
        limitedAccessAnswers: Bool,
        _ send: @escaping @Sendable () async throws -> RequestOutcome
    ) -> Task<Void, Never> {
        requestsUnderWay += 1
        return Task {
            defer { requestsUnderWay -= 1 }
            do {
                let outcome = try await send()
                if limitedAccessAnswers && outcome == .needsPermission {
                    return
                }
                if let message = UserText.failure(outcome) {
                    log.error("\(subject.description, privacy: .public): \(outcome, privacy: .public)")
                    prompt = .failure(title: failureTitle, message: message)
                }
            } catch {
                log.error("\(subject.description, privacy: .public): \(error: error)")
                prompt = .failure(title: failureTitle, message: UserText.describe(error))
            }
        }
    }

    // MARK: - Failures nobody asked about

    /// A report to the engine; its failures repeat with each cell, so the first since the last full
    /// reload is logged.
    private func call(_ subject: StaticString, _ body: () throws -> Void) {
        do {
            try body()
        } catch {
            if let apiError = error as? FeatureApiError, case .NotStarted = apiError { return }
            guard !callFailureLogged else { return }
            callFailureLogged = true
            log.notice("\(subject.description, privacy: .public): \(error: error); later report failures are not logged until the next start")
        }
    }

    /// Logs a failure the user did not start; before the engine starts there is nothing to report.
    private func record(_ error: any Error, _ subject: StaticString) {
        if let apiError = error as? FeatureApiError, case .NotStarted = apiError { return }
        log.error("\(subject.description, privacy: .public): \(error: error)")
    }
}

/// The file Quick Look shows of a photo — its full image once cached, until then its thumbnail, or
/// nothing while neither is — for one version of the photo.
private struct PreviewSource: Equatable {
    enum Kind {
        case nothing, thumbnail, fullImage
    }

    let id: Int64
    let kind: Kind
    let modifiedMs: Int64
}

/// What the file queue did for a Quick Look request.
private enum PreviewWrite: Sendable {
    /// A newer request came before this one's turn.
    case skipped
    /// The file written (`nil`: no photo ImageIO reads), and whether it is the full image.
    case written(file: URL?, isFullImage: Bool)
    /// The full image is cached but is no photo ImageIO reads, so Quick Look is not handed it; the
    /// thumbnail's file instead (`nil`: none, or no photo either).
    case fullImageNoPhoto(thumbnail: URL?)

    /// The request did its work, which may have left a file in Quick Look's folder.
    var wroteFiles: Bool {
        switch self {
        case .skipped: false
        case .written, .fullImageNoPhoto: true
        }
    }
}

/// No app on this Mac opens the photo's type.
struct PhotosNoAppToOpen: LocalizedError {
    var errorDescription: String? {
        String(localized: "No app on this Mac can open it.")
    }
}
