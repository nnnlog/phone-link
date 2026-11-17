import AppKit
import Foundation
import ImageIO
import PhoneLinkEngine
import Testing
import UniformTypeIdentifiers
@testable import PhoneLinkKit

@MainActor
@Suite struct PhotosModelTests {
    // MARK: Loading and states

    @Test func loadsStateAndGalleryWhenTheEngineStarts() {
        let rig = PhotosRig()
        defer { rig.removeFiles() }
        rig.api.update { $0.gallery = [photo(3), photo(1), photo(2)] }
        rig.hub.postStarted()
        #expect(rig.model.items.map(\.id) == [3, 1, 2])
        #expect(rig.model.state == photosState())
        #expect(rig.model.content == .gallery)
    }

    @Test func showsNothingBeforeTheEngineStarts() {
        let rig = PhotosRig()
        defer { rig.removeFiles() }
        rig.api.update { $0.failure = .NotStarted }
        rig.hub.postStarted()
        #expect(rig.model.state == nil)
        #expect(rig.model.content == .nothing)
        #expect(rig.model.prompt == nil)
    }

    @Test func contentFollowsTheStateInOrderOfPrecedence() {
        let rig = PhotosRig()
        defer { rig.removeFiles() }
        rig.api.update { $0.gallery = [photo(1)] }
        rig.hub.postStarted()

        let cases: [(PhotosState, PhotosContent)] = [
            (photosState(enabled: false, consent: false, permission: false), .unavailable),
            (photosState(consent: false, permission: false), .turnedOff),
            (photosState(permission: false), .needsPhoneAccess),
            (photosState(), .gallery),
        ]
        for (state, content) in cases {
            rig.api.update { $0.state = state }
            rig.hub.post(.photos(change: .stateChanged))
            #expect(rig.model.content == content)
        }

        rig.api.update { $0.gallery = [] }
        rig.hub.post(.photos(change: .galleryChanged))
        #expect(rig.model.content == .empty)
    }

    // MARK: Thumbnails

    @Test func aCellThatAppearsGetsItsThumbnail() async {
        let rig = PhotosRig()
        defer { rig.removeFiles() }
        rig.api.update {
            $0.gallery = [photo(1)]
            $0.thumbnails[1] = png()
        }
        rig.hub.postStarted()

        rig.model.itemAppeared(1)
        #expect(rig.api.calls.contains(.itemVisible(1)))
        #expect(await eventually { rig.item(1)?.thumbnail != nil })
        #expect(rig.api.calls.contains(.thumbnail(1)))
    }

    @Test func anItemWithoutThumbnailShowsItsFullImageScaledDown() async {
        let rig = PhotosRig()
        defer { rig.removeFiles() }
        rig.api.update {
            $0.gallery = [photo(2, thumbnail: false, full: true)]
            $0.fullImages[2] = png(width: 64, height: 48)
        }
        rig.hub.postStarted()

        rig.model.itemAppeared(2)
        #expect(await eventually { rig.item(2)?.thumbnail != nil })
        #expect(rig.api.calls.contains(.fullImage(2)))
        #expect(!rig.api.calls.contains(.thumbnail(2)))
    }

    @Test func galleryChangesDropUnlistedItemsAndRefreshChangedThumbnails() async {
        let rig = PhotosRig()
        defer { rig.removeFiles() }
        rig.api.update {
            $0.gallery = [photo(1), photo(2), photo(3)]
            for id: Int64 in [1, 2, 3] { $0.thumbnails[id] = png() }
        }
        rig.hub.postStarted()

        for id: Int64 in [1, 2, 3] { rig.model.itemAppeared(id) }
        #expect(await eventually { rig.model.items.allSatisfy { $0.thumbnail != nil } })

        // 1 left the phone's list, 2 was edited, 3 is unchanged.
        rig.api.update { $0.gallery = [photo(2, modified: 2), photo(3)] }
        rig.hub.post(.photos(change: .galleryChanged))
        #expect(rig.model.items.map(\.id) == [2, 3])
        #expect(rig.item(1) == nil)
        #expect(await eventually { rig.api.calls.filter { $0 == .thumbnail(2) }.count == 2 })
        #expect(rig.api.calls.filter { $0 == .thumbnail(3) }.count == 1)
        #expect(rig.item(2)?.thumbnail != nil)
    }

    @Test func thumbnailsOfCellsOffScreenAreEvictedBeyondTheLimit() async {
        let rig = PhotosRig(thumbnailLimit: 2)
        defer { rig.removeFiles() }
        rig.api.update {
            $0.gallery = [photo(1), photo(2), photo(3)]
            for id: Int64 in [1, 2, 3] { $0.thumbnails[id] = png() }
        }
        rig.hub.postStarted()

        for id: Int64 in [1, 2] {
            rig.model.itemAppeared(id)
            #expect(await eventually { rig.item(id)?.thumbnail != nil })
            rig.model.itemDisappeared(id)
        }

        rig.model.itemAppeared(3)
        #expect(await eventually { rig.item(3)?.thumbnail != nil })
        #expect(rig.item(1)?.thumbnail == nil)
        #expect(rig.item(2)?.thumbnail != nil)
    }

    @Test func leavingTheGalleryLetsGoOfTheThumbnailsOfCellsOffScreen() async {
        let rig = PhotosRig()
        defer { rig.removeFiles() }
        rig.api.update {
            $0.gallery = [photo(1), photo(2), photo(3)]
            for id: Int64 in [1, 2, 3] { $0.thumbnails[id] = png() }
        }
        rig.hub.postStarted()

        for id: Int64 in [1, 2, 3] { rig.model.itemAppeared(id) }
        #expect(await eventually { rig.model.items.allSatisfy { $0.thumbnail != nil } })

        rig.model.itemDisappeared(1)
        #expect(rig.item(1)?.thumbnail != nil, "scrolled away: cached within the limit")

        // The window closes with 2 and 3 on screen.
        rig.model.galleryLeft()
        #expect(rig.item(1)?.thumbnail == nil)
        #expect(rig.item(2)?.thumbnail != nil && rig.item(3)?.thumbnail != nil)

        // Another section: the cells go, and their thumbnails with them.
        rig.model.itemDisappeared(2)
        #expect(rig.item(2)?.thumbnail == nil)

        rig.model.galleryEntered()
        rig.model.itemAppeared(1)
        #expect(await eventually { rig.item(1)?.thumbnail != nil })
    }

    // MARK: Fetch triggers

    @Test func cellsAndTheGalleryReportWhatIsOnScreen() async {
        let rig = PhotosRig()
        defer { rig.removeFiles() }
        rig.api.update { $0.gallery = [photo(1), photo(2), photo(3)] }
        rig.hub.postStarted()

        rig.model.itemAppeared(3)
        rig.model.itemAppeared(1)
        await rig.model.layoutReport?.value
        #expect(rig.api.calls.contains(.gridLayout([1, 3])))

        rig.model.itemDisappeared(1)
        #expect(rig.api.calls.contains(.itemHidden(1)))
        rig.model.contextMenuOpened(3)
        #expect(rig.api.calls.contains(.contextMenuOpened(3)))

        rig.model.galleryLeft()
        #expect(rig.api.calls.last == .galleryLeft)

        rig.api.update { $0.calls = [] }
        rig.model.galleryEntered()
        #expect(rig.api.calls == [.itemVisible(3)])
    }

    @Test func arrowKeysMoveTheSelectionThroughTheGallery() {
        let rig = PhotosRig()
        defer { rig.removeFiles() }
        rig.api.update { $0.gallery = [photo(1), photo(2), photo(3), photo(4)] }
        rig.hub.postStarted()

        rig.model.moveSelection(by: 1)
        #expect(rig.model.focus == 1 && rig.model.selection == [1])
        rig.model.moveSelection(by: 2)
        #expect(rig.model.focus == 3 && rig.model.selection == [3])
        rig.model.moveSelection(by: 4)
        #expect(rig.model.focus == 3)
        rig.model.moveSelection(by: -1)
        #expect(rig.model.focus == 2 && rig.model.selection == [2])

        // From a selection of several, the arrows move from the last photo clicked.
        rig.model.selectAll()
        rig.model.moveSelection(by: 1)
        #expect(rig.model.focus == 3 && rig.model.selection == [3])

        rig.api.update { $0.gallery = [photo(1)] }
        rig.hub.post(.photos(change: .galleryChanged))
        #expect(rig.model.focus == nil)
        #expect(rig.model.selection.isEmpty)
    }

    // MARK: Selection

    @Test func clicksSelectOneCommandClicksToggleAndShiftClicksExtendFromTheAnchor() {
        let rig = PhotosRig()
        defer { rig.removeFiles() }
        rig.api.update { $0.gallery = (1...6).map { photo(Int64($0)) } }
        rig.hub.postStarted()

        rig.model.select(2)
        #expect(rig.model.selection == [2] && rig.model.focus == 2)
        rig.model.toggleSelection(4)
        #expect(rig.model.selection == [2, 4] && rig.model.focus == 4)

        // The range runs from the last Command-click and adds to what was selected then.
        rig.model.extendSelection(to: 6)
        #expect(rig.model.selection == [2, 4, 5, 6] && rig.model.focus == 6)

        // A second Shift-click replaces the first one's range.
        rig.model.extendSelection(to: 3)
        #expect(rig.model.selection == [2, 3, 4] && rig.model.focus == 3)
        #expect(rig.model.selectedIDs == [2, 3, 4], "in gallery order")

        // Command-click on a selected photo removes it; the focus goes with it.
        rig.model.toggleSelection(3)
        #expect(rig.model.selection == [2, 4] && rig.model.focus == nil)
        rig.model.extendSelection(to: 1)
        #expect(rig.model.selection == [1, 2, 3, 4] && rig.model.focus == 1)

        // A click starts over.
        rig.model.select(5)
        rig.model.extendSelection(to: 3)
        #expect(rig.model.selection == [3, 4, 5])

        // A photo that leaves the gallery leaves the selection.
        rig.api.update { $0.gallery = ([1, 2, 3, 5, 6] as [Int64]).map { photo($0) } }
        rig.hub.post(.photos(change: .galleryChanged))
        #expect(rig.model.selectedIDs == [3, 5])
        #expect(rig.model.focus == 3)
    }

    @Test func aShiftClickWithoutAnAnchorSelectsThePhotoAlone() {
        let rig = PhotosRig()
        defer { rig.removeFiles() }
        rig.api.update { $0.gallery = [photo(1), photo(2), photo(3)] }
        rig.hub.postStarted()

        rig.model.extendSelection(to: 2)
        #expect(rig.model.selection == [2] && rig.model.focus == 2)

        // The anchor's photo leaves the gallery.
        rig.api.update { $0.gallery = [photo(1), photo(3)] }
        rig.hub.post(.photos(change: .galleryChanged))
        rig.model.extendSelection(to: 3)
        #expect(rig.model.selection == [3])
    }

    @Test func selectAllSelectsEveryPhotoAndTheContextMenuActsOnTheSelectionItIsPartOf() {
        let rig = PhotosRig()
        defer { rig.removeFiles() }
        rig.api.update { $0.gallery = [photo(1), photo(2), photo(3)] }
        rig.hub.postStarted()

        rig.model.select(2)
        rig.model.selectAll()
        #expect(rig.model.selection == [1, 2, 3])
        #expect(rig.model.focus == 2, "the focus stays")
        #expect(rig.model.targets(clicked: 1) == [1, 2, 3])

        rig.model.select(1)
        rig.model.toggleSelection(2)
        #expect(rig.model.targets(clicked: 2) == [1, 2])
        #expect(rig.model.targets(clicked: 3) == [3])
    }

    @Test func quickLookFollowsTheLastPhotoClicked() async {
        let rig = PhotosRig()
        defer { rig.removeFiles() }
        rig.api.update {
            $0.gallery = [photo(1), photo(2), photo(3)]
            for id: Int64 in [1, 2, 3] { $0.thumbnails[id] = png() }
        }
        rig.hub.postStarted()

        rig.model.select(1)
        rig.model.toggleSelection(2)
        rig.model.toggleViewer()
        #expect(rig.api.viewerCalls.first == .openItem(2))
        #expect(rig.model.selection == [1, 2], "opening Quick Look keeps the selection")
        await rig.model.previewLoad?.value
        #expect(rig.model.preview?.id == 2)

        rig.model.extendSelection(to: 3)
        await rig.model.previewLoad?.value
        #expect(rig.model.preview?.id == 3)

        // The photo it shows leaves the selection: it closes.
        rig.model.toggleSelection(3)
        #expect(rig.model.preview == nil)
        #expect(rig.api.viewerCalls.last == .viewerClosed)
    }

    // MARK: Quick Look

    @Test func quickLookShowsTheThumbnailUntilTheFullImageArrives() async throws {
        let rig = PhotosRig()
        defer { rig.removeFiles() }
        let full = jpeg()
        rig.api.update {
            $0.gallery = [photo(1), photo(2)]
            $0.thumbnails[1] = png()
            $0.thumbnails[2] = png()
            $0.names[1] = PhotoFileName(stem: "IMG_0001", extension: ".jpg")
        }
        rig.hub.postStarted()

        rig.model.openViewer(1)
        #expect(rig.api.viewerCalls == [.openItem(1), .viewerSelection(1), .viewerItemShown(1)])
        await rig.model.previewLoad?.value
        let thumbnail = try #require(rig.model.preview)
        #expect(thumbnail.id == 1 && !thumbnail.isFullImage)
        #expect(thumbnail.title == "IMG_0001.jpg")
        #expect(thumbnail.file?.lastPathComponent == "IMG_0001.png")

        rig.api.update {
            $0.fullImages[1] = full
            $0.gallery = [photo(1, full: true), photo(2)]
        }
        rig.hub.post(.photos(change: .fullImageStored(id: 1)))
        await rig.model.previewLoad?.value
        let fullPreview = try #require(rig.model.preview)
        #expect(fullPreview.isFullImage)
        #expect(fullPreview.file?.lastPathComponent == "IMG_0001.jpg")
        #expect(try Data(contentsOf: #require(fullPreview.file)) == full)

        // The gallery change that follows the stored image changes nothing.
        rig.hub.post(.photos(change: .galleryChanged))
        #expect(rig.model.preview == fullPreview)

        rig.model.select(2)
        #expect(rig.api.viewerCalls.suffix(2) == [.viewerSelection(2), .viewerItemShown(2)])
        await rig.model.previewLoad?.value
        #expect(rig.model.preview?.id == 2)

        rig.model.closeViewer()
        #expect(rig.model.preview == nil)
        #expect(rig.api.viewerCalls.last == .viewerClosed)

        rig.model.select(1)
        #expect(rig.model.preview == nil)
        #expect(rig.api.fullImageReadsOnMainThread == 0)
    }

    @Test func quickLookRemovesItsFilesAsItMovesOn() async throws {
        let rig = PhotosRig()
        defer { rig.removeFiles() }
        rig.api.update {
            $0.gallery = [photo(1, full: true), photo(2)]
            $0.fullImages[1] = jpeg()
            $0.thumbnails[2] = png()
        }
        rig.hub.postStarted()

        rig.model.openViewer(1)
        await rig.model.previewLoad?.value
        let first = try #require(rig.model.preview?.file)
        #expect(FileManager.default.fileExists(atPath: first.path))

        rig.model.select(2)
        await rig.model.previewLoad?.value
        let second = try #require(rig.model.preview?.file)
        #expect(await eventually { !FileManager.default.fileExists(atPath: first.path) })
        #expect(FileManager.default.fileExists(atPath: second.path))

        rig.model.closeViewer()
        #expect(await eventually { !FileManager.default.fileExists(atPath: second.path) })
    }

    @Test func quickLookShowsTheNewVersionOfAPhotoEditedOnThePhone() async throws {
        let rig = PhotosRig()
        defer { rig.removeFiles() }
        rig.api.update {
            $0.gallery = [photo(1, full: true)]
            $0.fullImages[1] = jpeg()
        }
        rig.hub.postStarted()

        rig.model.openViewer(1)
        await rig.model.previewLoad?.value
        let before = try #require(rig.model.preview)

        let edited = jpeg(width: 16, height: 12)
        rig.api.update {
            $0.gallery = [photo(1, full: true, modified: 2)]
            $0.fullImages[1] = edited
        }
        rig.hub.post(.photos(change: .galleryChanged))
        await rig.model.previewLoad?.value
        let after = try #require(rig.model.preview)
        #expect(after != before)
        #expect(try Data(contentsOf: #require(after.file)) == edited)
    }

    @Test func aPhotoThatLeftTheGalleryIsNeitherSelectedNorOpened() async {
        let rig = PhotosRig()
        defer { rig.removeFiles() }
        rig.api.update {
            $0.gallery = [photo(1), photo(2), photo(3)]
            for id: Int64 in [1, 2, 3] { $0.thumbnails[id] = png() }
        }
        rig.hub.postStarted()

        rig.model.openViewer(2)
        await rig.model.previewLoad?.value

        // 1 leaves the gallery under a resting pointer: deleted from this Mac, the engine keeps its
        // row (and the fake still opens it).
        rig.api.update {
            $0.gallery = [photo(2), photo(3)]
            $0.calls = []
        }
        rig.hub.post(.photos(change: .galleryChanged))

        // A right-click on the stale cell, with Quick Look open.
        rig.model.select(1)
        rig.model.contextMenuOpened(1)
        #expect(rig.model.focus == 2 && rig.model.selection == [2])
        #expect(rig.model.preview?.id == 2)

        // And with it closed.
        rig.model.closeViewer()
        rig.model.openViewer(1)
        #expect(rig.model.preview == nil)
        #expect(rig.model.focus == 2 && rig.model.selection == [2])
        #expect(rig.api.calls == [.viewerClosed])
    }

    @Test func quickLookStaysClosedForAnItemWithoutImage() {
        let rig = PhotosRig()
        defer { rig.removeFiles() }
        rig.api.update {
            $0.gallery = [photo(1, thumbnail: false)]
            $0.opens = false
        }
        rig.hub.postStarted()

        rig.model.openViewer(1)
        #expect(rig.model.preview == nil)
        #expect(!rig.api.calls.contains(.viewerSelection(1)))
    }

    @Test func quickLookIsHandedOnlyPhotosNamedAfterTheirBytes() async throws {
        let rig = PhotosRig()
        defer { rig.removeFiles() }
        let photoBytes = jpeg()
        rig.api.update {
            $0.gallery = [photo(1, full: true), photo(2, full: true)]
            // A photo the phone names as a Terminal file: Quick Look gets it as the JPEG it is.
            $0.names[1] = PhotoFileName(stem: "Sneaky", extension: ".terminal")
            $0.fullImages[1] = photoBytes
            // Full bytes that are no photo: Quick Look stays on the thumbnail.
            $0.names[2] = PhotoFileName(stem: "IMG_0002", extension: ".jpg")
            $0.fullImages[2] = Data("#!/bin/sh\n".utf8)
            $0.thumbnails[2] = png()
        }
        rig.hub.postStarted()

        rig.model.openViewer(1)
        await rig.model.previewLoad?.value
        let first = try #require(rig.model.preview)
        #expect(first.isFullImage && first.title == "Sneaky.terminal")
        #expect(first.file?.lastPathComponent == "Sneaky.jpeg")
        #expect(try Data(contentsOf: #require(first.file)) == photoBytes)

        rig.model.select(2)
        await rig.model.previewLoad?.value
        let second = try #require(rig.model.preview)
        #expect(second.id == 2 && !second.isFullImage)
        #expect(second.file?.lastPathComponent == "IMG_0002.png", "the thumbnail")
        let viewerFolder = try #require(second.file).deletingLastPathComponent().deletingLastPathComponent()
        #expect(!FileManager.default.fileExists(atPath: viewerFolder.appendingPathComponent("IMG_0002.jpg").path))

        // The full image is not read again until the photo changes.
        let reads = rig.api.calls.filter { $0 == .fullImage(2) }.count
        rig.hub.post(.photos(change: .galleryChanged))
        await rig.model.previewLoad?.value
        #expect(rig.api.calls.filter { $0 == .fullImage(2) }.count == reads)
        #expect(rig.model.preview == second)
    }

    @Test func quickLookFileNamesFollowTheBytes() throws {
        let jpg = PhotoFileName(stem: "IMG", extension: ".jpg")
        #expect(try PhotosTemporaryFiles.viewerFileName(of: jpeg(), name: jpg) == "IMG.jpg", "the name agrees")
        #expect(try PhotosTemporaryFiles.viewerFileName(of: png(), name: jpg) == "IMG.png")
        let webloc = PhotoFileName(stem: "Link", extension: ".webloc")
        #expect(try PhotosTemporaryFiles.viewerFileName(of: png(), name: webloc) == "Link.png")

        // No photo: bytes ImageIO reads as none, an SVG, or a PDF (ImageIO detects it, but it is no image).
        #expect(try PhotosTemporaryFiles.viewerFileName(of: Data("not an image".utf8), name: jpg) == nil)
        let svg = Data(#"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"/>"#.utf8)
        let svgName = PhotoFileName(stem: "x", extension: ".svg")
        #expect(try PhotosTemporaryFiles.viewerFileName(of: svg, name: svgName) == nil)
        #expect(try PhotosTemporaryFiles.viewerFileName(of: pdf(), name: jpg) == nil)
        #expect(PhotosImageDecoder.type(of: pdf()) == .pdf)

        // A stem that names no single file is refused, as for every photo file.
        #expect(throws: PhotosUnusableFileName.self) {
            try PhotosTemporaryFiles.viewerFileName(of: png(), name: PhotoFileName(stem: "a/b", extension: ".png"))
        }
    }

    // MARK: Delete

    @Test func deleteAsksForConfirmationThenDeletes() async {
        let rig = PhotosRig()
        defer { rig.removeFiles() }
        rig.api.update { $0.gallery = [photo(1)] }
        rig.hub.postStarted()

        await rig.model.delete(1)?.value
        #expect(rig.model.prompt == .confirmDelete(ids: [1], name: "Photo1.jpg"))
        #expect(rig.model.prompt?.message == "“Photo1.jpg” will be deleted from your phone. This can't be undone.")
        #expect(rig.api.calls.contains(.prepareDelete(1)))

        await rig.model.confirmDelete([1]).value
        #expect(rig.api.calls.last == .delete(1))
    }

    @Test func deleteOfSeveralPhotosAsksOnceForThoseThatCanBeDeleted() async {
        let rig = PhotosRig()
        defer { rig.removeFiles() }
        rig.api.update { $0.gallery = [photo(1), photo(2), photo(3, deleteState: .deleting)] }
        rig.hub.postStarted()

        #expect(rig.model.deletable([1, 2, 3]) == [1, 2])
        await rig.model.delete([1, 2, 3])?.value
        #expect(rig.api.calls.filter { if case .prepareDelete = $0 { true } else { false } } == [
            .prepareDelete(1), .prepareDelete(2),
        ])
        let prompt = rig.model.prompt
        #expect(prompt == .confirmDelete(ids: [1, 2], name: ""))
        #expect(prompt?.title == "Delete 2 Photos from Your Phone?")
        #expect(prompt?.message == "These 2 photos will be deleted from your phone. This can't be undone.")

        await rig.model.confirmDelete([1, 2]).value
        #expect(rig.api.calls.suffix(2) == [.delete(1), .delete(2)])
    }

    @Test func noConfirmationShowsForAPhotoThatLeftTheGalleryDuringThePreCheck() async {
        let rig = PhotosRig()
        defer { rig.removeFiles() }
        rig.api.update { $0.gallery = [photo(1), photo(2)] }
        rig.hub.postStarted()

        let deleting = rig.model.delete(1)
        rig.api.update { $0.gallery = [photo(2)] }
        rig.hub.post(.photos(change: .galleryChanged))
        await deleting?.value
        #expect(rig.api.calls.contains(.prepareDelete(1)))
        #expect(rig.model.prompt == nil)
    }

    @Test func deleteWithoutPhonePermissionOffersToAskThePhone() async {
        let rig = PhotosRig()
        defer { rig.removeFiles() }
        rig.api.update {
            $0.gallery = [photo(1)]
            $0.deleteCheck = .missingPermission
            $0.outcome = .denied
        }
        rig.hub.postStarted()

        await rig.model.delete(1)?.value
        #expect(rig.model.prompt == .allowDeleting)

        await rig.model.allowDeletingOnPhone().value
        #expect(rig.api.calls.last == .requestDeletePermission)
        #expect(rig.model.prompt == .failure(
            title: "Couldn't Allow Deleting Photos",
            message: "Your phone denied the request."
        ))
    }

    @Test func deleteIsNotOfferedWhenThePhoneOrTheItemCannotDelete() {
        let rig = PhotosRig()
        defer { rig.removeFiles() }
        rig.api.update {
            $0.gallery = [photo(1), photo(2, thumbnail: false), photo(3, deleteState: .deleting)]
            $0.state = photosState(deleteAvailable: false)
        }
        rig.hub.postStarted()

        #expect(rig.model.delete(1) == nil)

        rig.api.update { $0.state = photosState() }
        rig.hub.post(.photos(change: .stateChanged))
        #expect(rig.model.delete(2) == nil)
        #expect(rig.model.delete(3) == nil)
        #expect(!rig.api.calls.contains { if case .prepareDelete = $0 { true } else { false } })
    }

    @Test func aFailedDeleteShowsTheNotice() {
        let rig = PhotosRig()
        defer { rig.removeFiles() }
        rig.hub.post(.photos(change: .deleteFinished(result: PhotoDeleteResult(
            outcome: .success, deleted: [1], reverted: [], failed: false
        ))))
        #expect(rig.model.prompt == nil)

        rig.hub.post(.photos(change: .deleteFinished(result: PhotoDeleteResult(
            outcome: .success, deleted: [], reverted: [1], failed: true
        ))))
        #expect(rig.model.prompt == .failure(
            title: "Couldn't Delete the Photo",
            message: "The photo is still on your phone."
        ))
    }

    // MARK: File actions

    @Test func anActionWithoutTheFullImageSaysSo() async {
        let rig = PhotosRig()
        defer { rig.removeFiles() }
        rig.api.update { $0.gallery = [photo(1)] }
        rig.hub.postStarted()

        await rig.model.open(1)?.value
        #expect(rig.api.calls.contains(.fullImageForAction(1, .open)))
        #expect(rig.model.prompt == .failure(title: "Couldn't Open the Photo", message: "Your phone didn't respond."))
        #expect(rig.opened.isEmpty)
        #expect(rig.item(1)?.isBusy == false)
    }

    @Test func actionFilesCarryTheSaveAsName() async throws {
        let rig = PhotosRig()
        defer { rig.removeFiles() }
        let bytes = Data("heic bytes".utf8)
        rig.api.update {
            $0.gallery = [photo(1)]
            $0.names[1] = PhotoFileName(stem: "IMG_0042", extension: ".heic")
            $0.actionImages[1] = bytes
        }
        rig.hub.postStarted()

        let pending = try #require(rig.model.startFile(1, for: .saveAs))
        #expect(pending.fileName == "IMG_0042.heic")
        #expect(pending.contentType == .heic)

        let file = try #require(await rig.model.file(from: pending))
        #expect(file.url.lastPathComponent == "IMG_0042.heic")
        #expect(file.url.path.hasPrefix(rig.folder.path))
        #expect(try Data(contentsOf: file.url) == bytes)

        let destination = rig.folder.appendingPathComponent("Saved.heic")
        await rig.model.save(pending, to: destination).value
        #expect(try Data(contentsOf: destination) == bytes)
        #expect(rig.api.calls.filter { $0 == .fullImageForAction(1, .saveAs) }.count == 1)
    }

    @Test func savingSeveralPhotosWritesEachIntoTheFolderNumberingTakenNames() async throws {
        let rig = PhotosRig()
        defer { rig.removeFiles() }
        rig.api.update {
            $0.gallery = [photo(1), photo(2), photo(3)]
            $0.names[1] = PhotoFileName(stem: "IMG", extension: ".jpg")
            $0.names[2] = PhotoFileName(stem: "IMG", extension: ".jpg")
            $0.names[3] = PhotoFileName(stem: "Other", extension: ".png")
            for id: Int64 in [1, 2, 3] { $0.actionImages[id] = Data("photo \(id)".utf8) }
        }
        rig.hub.postStarted()

        let folder = rig.folder.appendingPathComponent("Saved", isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        try Data("already there".utf8).write(to: folder.appendingPathComponent("IMG.jpg"))

        rig.api.holdActions()
        let saving = rig.model.save([1, 2, 3], toFolder: folder)
        #expect(await eventually { rig.api.heldActionCount == 1 })
        #expect(rig.model.items.allSatisfy { $0.isBusy }, "the photos still to come show as busy")
        #expect(rig.api.calls.filter { if case .fullImageForAction = $0 { true } else { false } } == [
            .fullImageForAction(1, .saveAs),
        ], "one photo at a time")
        rig.api.releaseActions()
        await saving.value

        let read = { (name: String) in try Data(contentsOf: folder.appendingPathComponent(name)) }
        #expect(try read("IMG.jpg") == Data("already there".utf8))
        #expect(try read("IMG 2.jpg") == Data("photo 1".utf8))
        #expect(try read("IMG 3.jpg") == Data("photo 2".utf8))
        #expect(try read("Other.png") == Data("photo 3".utf8))
        #expect(rig.model.prompt == nil)
        #expect(!rig.model.items.contains { $0.isBusy })
    }

    @Test func savingSeveralPhotosReportsThoseNotSavedInOneAlert() async throws {
        let rig = PhotosRig()
        defer { rig.removeFiles() }
        rig.api.update {
            $0.gallery = [photo(1), photo(2), photo(3)]
            // 2 and 3 don't arrive in time.
            $0.actionImages[1] = Data("photo 1".utf8)
        }
        rig.hub.postStarted()

        let folder = rig.folder.appendingPathComponent("Saved", isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)

        await rig.model.save([1, 2, 3], toFolder: folder).value
        #expect(try Data(contentsOf: folder.appendingPathComponent("Photo1.jpg")) == Data("photo 1".utf8))
        #expect(rig.api.calls.contains(.fullImageForAction(3, .saveAs)), "a failure doesn't stop the rest")
        #expect(rig.model.prompt == .failure(
            title: "Couldn't Save 2 Photos",
            message: "1 of 3 photos were saved. Your phone didn't respond."
        ))

        rig.model.prompt = nil
        await rig.model.save([2], toFolder: folder).value
        #expect(rig.model.prompt == .failure(title: "Couldn't Save the Photo", message: "Your phone didn't respond."))
    }

    @Test func numberedNamesFollowTheFinder() throws {
        let jpg = PhotoFileName(stem: "IMG_0001", extension: ".jpg")
        #expect(try PhotosModel.numberedFileName(jpg, number: 1) == "IMG_0001.jpg")
        #expect(try PhotosModel.numberedFileName(jpg, number: 2) == "IMG_0001 2.jpg")
        #expect(try PhotosModel.numberedFileName(PhotoFileName(stem: "Scan", extension: ""), number: 3) == "Scan 3")
        #expect(throws: PhotosUnusableFileName.self) {
            try PhotosModel.numberedFileName(PhotoFileName(stem: "a/b", extension: ".jpg"), number: 2)
        }
    }

    @Test func copyOfSeveralPhotosPutsEachOnThePasteboard() async throws {
        let rig = PhotosRig()
        defer { rig.removeFiles() }
        rig.api.update {
            $0.gallery = [photo(1), photo(2)]
            $0.actionImages[1] = Data("one".utf8)
            $0.actionImages[2] = Data("two".utf8)
        }
        rig.hub.postStarted()

        await rig.model.copy([1, 2])?.value
        let entries = try #require(rig.pasteboard.pasteboardItems)
        let names = entries.compactMap { $0.string(forType: .fileURL).flatMap(URL.init(string:))?.lastPathComponent }
        #expect(names == ["Photo1.jpg", "Photo2.jpg"])
        #expect(rig.model.prompt == nil)
    }

    @Test func aNameThatIsNoFileNameIsRefused() {
        let rig = PhotosRig()
        defer { rig.removeFiles() }
        rig.api.update {
            $0.gallery = [photo(1)]
            $0.names[1] = PhotoFileName(stem: "../evil", extension: ".jpg")
        }
        rig.hub.postStarted()

        #expect(rig.model.startFile(1, for: .copy) == nil)
        #expect(rig.model.prompt?.title == "Couldn't Copy the Photo")
        #expect(!rig.api.calls.contains(.fullImageForAction(1, .copy)))
    }

    @Test func openWithUsesTheChosenApp() async {
        let rig = PhotosRig()
        defer { rig.removeFiles() }
        let app = URL(fileURLWithPath: "/Applications/Preview.app")
        rig.api.update {
            $0.gallery = [photo(1)]
            $0.actionImages[1] = Data("jpeg".utf8)
        }
        rig.hub.postStarted()

        await rig.model.open(1, with: app)?.value
        #expect(rig.api.calls.contains(.fullImageForAction(1, .openWith)))
        #expect(rig.opened.map(\.app) == [app])
        #expect(rig.opened.first?.file.lastPathComponent == "Photo1.jpg")
    }

    @Test func copyPutsTheFileAndTheImageOnThePasteboard() async throws {
        let rig = PhotosRig()
        defer { rig.removeFiles() }
        let pasteboard = rig.pasteboard
        let bytes = Data("jpeg bytes".utf8)
        rig.api.update {
            $0.gallery = [photo(1)]
            $0.actionImages[1] = bytes
        }
        rig.hub.postStarted()

        await rig.model.copy(1)?.value
        let url = try #require(pasteboard.string(forType: .fileURL).flatMap(URL.init(string:)))
        #expect(url.lastPathComponent == "Photo1.jpg")
        #expect(try Data(contentsOf: url) == bytes)
        #expect(pasteboard.data(forType: NSPasteboard.PasteboardType(UTType.jpeg.identifier)) == bytes)
        #expect(rig.model.prompt == nil)
    }

    @Test func aClickedNewPhotoNotificationOpensTheFullImage() async throws {
        let rig = PhotosRig()
        defer { rig.removeFiles() }
        let bytes = Data("new photo".utf8)
        rig.api.update {
            $0.fullImages[5] = bytes
            $0.names[5] = PhotoFileName(stem: "PXL_20261021", extension: ".jpg")
        }

        rig.hub.post(.photos(change: .openImage(id: 5)))
        #expect(await eventually { rig.opened.count == 1 })
        let opened = try #require(rig.opened.first)
        #expect(opened.app == nil)
        #expect(opened.file.lastPathComponent == "PXL_20261021.jpg")
        #expect(try Data(contentsOf: opened.file) == bytes)
        #expect(rig.api.fullImageReadsOnMainThread == 0)
    }

    @Test func aCopyThatArrivesAfterTheClipboardChangedLeavesItAlone() async {
        let rig = PhotosRig()
        defer { rig.removeFiles() }
        rig.api.update {
            $0.gallery = [photo(1)]
            $0.actionImages[1] = Data("jpeg".utf8)
        }
        rig.hub.postStarted()

        rig.api.holdActions()
        let copying = rig.model.copy(1)
        #expect(await eventually { rig.api.heldActionCount == 1 })

        // The user copies something else while the image is on its way.
        rig.pasteboard.clearContents()
        rig.pasteboard.setString("newer", forType: .string)

        rig.api.releaseActions()
        await copying?.value
        #expect(rig.pasteboard.string(forType: .string) == "newer")
        #expect(rig.pasteboard.string(forType: .fileURL) == nil)
        #expect(rig.model.prompt == nil)
    }

    @Test func anOpenThatArrivesAfterTheGalleryWasLeftIsDropped() async {
        let rig = PhotosRig()
        defer { rig.removeFiles() }
        rig.api.update {
            $0.gallery = [photo(1), photo(2)]
            $0.actionImages[1] = Data("jpeg".utf8)
        }
        rig.hub.postStarted()

        rig.api.holdActions()
        let opening = rig.model.open(1)
        let failing = rig.model.open(2)
        #expect(await eventually { rig.api.heldActionCount == 2 })

        // The user went to another section and came back.
        rig.model.galleryLeft()
        rig.model.galleryEntered()

        rig.api.releaseActions()
        await opening?.value
        await failing?.value
        #expect(rig.opened.isEmpty)
        #expect(rig.model.prompt == nil, "a late failure is dropped too")

        // While the gallery stays, Open goes ahead, also after the selection moved.
        rig.model.select(1)
        let staying = rig.model.open(1)
        rig.model.select(2)
        await staying?.value
        #expect(rig.opened.map(\.file.lastPathComponent) == ["Photo1.jpg"])
    }

    @Test func aShareThatArrivesAfterTheUserMovedOnIsDropped() async throws {
        let rig = PhotosRig()
        defer { rig.removeFiles() }
        rig.api.update {
            $0.gallery = [photo(1), photo(2)]
            $0.actionImages[1] = Data("jpeg".utf8)
        }
        rig.hub.postStarted()

        rig.model.select(1)
        let moved = try #require(rig.model.startFile(1, for: .share))
        rig.model.select(2)
        #expect(await rig.model.fileToShare(from: moved) == nil)

        rig.model.select(1)
        let left = try #require(rig.model.startFile(1, for: .share))
        rig.model.galleryLeft()
        #expect(await rig.model.fileToShare(from: left) == nil)
        #expect(rig.model.prompt == nil)

        rig.model.galleryEntered()
        let stayed = try #require(rig.model.startFile(1, for: .share))
        let file = try #require(await rig.model.fileToShare(from: stayed))
        #expect(file.url.lastPathComponent == "Photo1.jpg")
    }

    @Test func aPhotoThatLeavesTheGalleryOrChangesLosesItsFiles() async throws {
        let rig = PhotosRig()
        defer { rig.removeFiles() }
        rig.api.update {
            $0.gallery = [photo(1), photo(2), photo(3, full: true)]
            for id: Int64 in [1, 2, 3] { $0.actionImages[id] = Data("photo \(id)".utf8) }
        }
        rig.hub.postStarted()

        var written: [Int64: URL] = [:]
        for id: Int64 in [1, 2, 3] {
            let pending = try #require(rig.model.startFile(id, for: .open))
            written[id] = try #require(await rig.model.file(from: pending)).url
        }

        let urls = written
        let exists = { (id: Int64) in FileManager.default.fileExists(atPath: urls[id]!.path) }

        // 2 is edited on the phone, and the engine drops its copy of 3's full image.
        rig.api.update { $0.gallery = [photo(1), photo(2, modified: 2), photo(3)] }
        rig.hub.post(.photos(change: .galleryChanged))
        #expect(await eventually { !exists(2) && !exists(3) })
        #expect(exists(1), "an unchanged photo keeps the file an app may still use")

        // 1 leaves the gallery: deleted from this Mac, or removed on the phone.
        rig.api.update { $0.gallery = [photo(2, modified: 2), photo(3)] }
        rig.hub.post(.photos(change: .galleryChanged))
        #expect(await eventually { !exists(1) })
    }

    @Test func saveAsWritesOnlyTheChosenFile() async throws {
        let rig = PhotosRig()
        defer { rig.removeFiles() }
        rig.api.update {
            $0.gallery = [photo(1)]
            $0.actionImages[1] = Data("jpeg".utf8)
        }
        rig.hub.postStarted()

        let pending = try #require(rig.model.startFile(1, for: .saveAs))
        try FileManager.default.createDirectory(at: rig.folder, withIntermediateDirectories: true)
        let destination = rig.folder.appendingPathComponent("Saved.jpg")
        await rig.model.save(pending, to: destination).value

        #expect(rig.model.prompt == nil)
        #expect(try Data(contentsOf: destination) == Data("jpeg".utf8))

        let photosRoot = rig.folder.appendingPathComponent("\(PhoneLinkIdentity.bundleID).photos")
        let copies = FileManager.default.enumerator(at: photosRoot, includingPropertiesForKeys: [.isRegularFileKey])?
            .compactMap { $0 as? URL }
            .filter { (try? $0.resourceValues(forKeys: [.isRegularFileKey]).isRegularFile) == true } ?? []
        #expect(copies.isEmpty, "no temporary copy")
    }

    @Test func photoAndVideoTypesAreHandedOnButNotScriptableOnes() {
        #expect(PhotosModel.isPhotoOrVideo(.jpeg) && PhotosModel.isPhotoOrVideo(.heic) && PhotosModel.isPhotoOrVideo(.mpeg4Movie))
        #expect(!PhotosModel.isPhotoOrVideo(.svg))
        #expect(!PhotosModel.isPhotoOrVideo(.html) && !PhotosModel.isPhotoOrVideo(.shellScript))
    }

    @Test func onlyPhotosAndVideosAreHandedToAnApp() async {
        let rig = PhotosRig()
        defer { rig.removeFiles() }
        rig.api.update {
            $0.gallery = [photo(1), photo(2)]
            $0.names[1] = PhotoFileName(stem: "Sneaky", extension: ".terminal")
            $0.names[2] = PhotoFileName(stem: "VID_0002", extension: ".mp4")
            $0.actionImages[1] = Data("settings".utf8)
            $0.actionImages[2] = Data("movie".utf8)
        }
        rig.hub.postStarted()

        #expect(rig.model.open(1) == nil)
        #expect(rig.model.prompt == .failure(
            title: "Couldn't Open the Photo",
            message: "“Sneaky.terminal” isn't a photo or video file."
        ))

        rig.model.prompt = nil
        #expect(rig.model.open(1, with: URL(fileURLWithPath: "/System/Applications/Utilities/Terminal.app")) == nil)
        #expect(rig.model.prompt?.title == "Couldn't Open the Photo")

        rig.model.prompt = nil
        #expect(rig.model.startFile(1, for: .dragOut) == nil)
        #expect(rig.model.prompt == nil, "a drag carries nothing, without an alert")
        #expect(rig.model.openWithContentType(of: 1) == nil, "Open With lists no app")
        #expect(!rig.api.calls.contains { if case .fullImageForAction = $0 { true } else { false } })

        // Copy, Save As and Share hand no file to the app its type names.
        #expect(rig.model.startFile(1, for: .copy) != nil)

        // A video opens.
        #expect(rig.model.openWithContentType(of: 2) == .mpeg4Movie)
        await rig.model.open(2)?.value
        #expect(rig.opened.map(\.file.lastPathComponent) == ["VID_0002.mp4"])

        // A clicked new-photo notification of such a name opens nothing either.
        rig.api.update {
            $0.names[5] = PhotoFileName(stem: "Sneaky", extension: ".webloc")
            $0.fullImages[5] = Data("url".utf8)
        }
        rig.hub.post(.photos(change: .openImage(id: 5)))
        #expect(!rig.api.calls.contains(.fullImage(5)))
        #expect(rig.opened.count == 1)
    }

    // MARK: Permission requests

    @Test func limitedPhotoAccessIsAnAnswerNotAFailure() async {
        let rig = PhotosRig()
        defer { rig.removeFiles() }
        rig.api.update { $0.outcome = .needsPermission }
        await rig.model.requestPhotoAccess().value
        #expect(rig.api.calls == [.requestPhotoAccess])
        #expect(rig.model.prompt == nil)
        #expect(!rig.model.requestPending)

        rig.api.update { $0.outcome = .timedOut }
        await rig.model.requestFullAccess().value
        #expect(rig.api.calls.last == .requestFullAccess)
        #expect(rig.model.prompt == .failure(
            title: "Couldn't Get Full Photo Access",
            message: "Your phone didn't respond."
        ))
    }

    @Test func turningOnAndDismissingReachTheEngine() {
        let rig = PhotosRig()
        defer { rig.removeFiles() }
        rig.model.turnOn()
        rig.model.dismissFullAccessReminder()
        #expect(rig.api.calls == [.setConsent(true), .dismissFullAccessReminder])
    }

    // MARK: Temporary files

    @Test func aLaunchRemovesTheFilesOfEarlierRuns() throws {
        let base = FileManager.default.temporaryDirectory
            .appendingPathComponent("PhotosModelTests-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: base) }
        let earlier = base
            .appendingPathComponent("\(PhoneLinkIdentity.bundleID).photos/earlier-run", isDirectory: true)
        try FileManager.default.createDirectory(at: earlier, withIntermediateDirectories: true)
        try Data("left behind".utf8).write(to: earlier.appendingPathComponent("IMG.jpg"))

        let files = PhotosTemporaryFiles(base: base)
        #expect(!FileManager.default.fileExists(atPath: earlier.path))

        let name = PhotoFileName(stem: "IMG", extension: ".jpg")
        let written = try files.writeFullImage(Data("x".utf8), id: 9, name: name)
        #expect(written.deletingLastPathComponent().deletingLastPathComponent().path == files.folder.path)

        files.removeAll()
        #expect(!FileManager.default.fileExists(atPath: files.folder.path))
    }

    @Test func quickLookFilesLiveApartFromActionFiles() async throws {
        let base = FileManager.default.temporaryDirectory
            .appendingPathComponent("PhotosModelTests-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: base) }

        let files = PhotosTemporaryFiles(base: base)
        let name = PhotoFileName(stem: "IMG", extension: ".jpg")
        let action = try await files.perform { try files.writeFullImage(Data("action".utf8), id: 9, name: name) }
        let photo = jpeg()
        let viewer = try #require(try await files.perform { try files.writeViewerImage(photo, id: 9, name: name) })
        #expect(action != viewer)
        let exists = { (url: URL) in FileManager.default.fileExists(atPath: url.path) }

        // Removals run after the work asked for before them, and before the work asked for after.
        files.removeViewerFiles(of: 9)
        try await files.perform {}
        #expect(!exists(viewer) && exists(action), "an app may still use the action's file")

        files.removeFiles(of: 9)
        try await files.perform {}
        #expect(!exists(action))
    }
}

// MARK: - Rig

@MainActor
private final class PhotosRig {
    let api = PhotosFakeApi()
    let hub = EngineEventHub()
    let folder = FileManager.default.temporaryDirectory.appendingPathComponent("PhotosModelTests-\(UUID().uuidString)")
    let pasteboard = NSPasteboard(name: NSPasteboard.Name("PhotosModelTests-\(UUID().uuidString)"))
    private(set) var opened: [(file: URL, app: URL?)] = []
    private(set) var model: PhotosModel!

    init(thumbnailLimit: Int = 240) {
        model = PhotosModel(
            api: api,
            hub: hub,
            temporaryDirectory: folder,
            pasteboard: pasteboard,
            openFile: { [weak self] file, app in self?.opened.append((file, app)) },
            thumbnailLimit: thumbnailLimit
        )
    }

    func item(_ id: Int64) -> PhotosGridItem? {
        model.items.first { $0.id == id }
    }

    func removeFiles() {
        try? FileManager.default.removeItem(at: folder)
        pasteboard.releaseGlobally()
    }
}

/// Polls until `condition` holds (thumbnails decode off the main thread), for at most 2 s.
@MainActor
private func eventually(_ condition: () -> Bool) async -> Bool {
    for _ in 0..<400 {
        if condition() { return true }
        try? await Task.sleep(for: .milliseconds(5))
    }
    return condition()
}

private func photo(
    _ id: Int64,
    thumbnail: Bool = true,
    full: Bool = false,
    modified: Int64 = 1,
    deleteState: PhotoDeleteState = .none
) -> Photo {
    Photo(
        id: id,
        name: "Photo\(id).jpg",
        takenMs: 1_000 - id,
        modifiedMs: modified,
        mimeType: "image/jpeg",
        width: 4,
        height: 4,
        size: 100,
        hasThumbnail: thumbnail,
        hasFull: full,
        deleteState: deleteState
    )
}

private func photosState(
    enabled: Bool = true,
    consent: Bool = true,
    permission: Bool = true,
    reminder: Bool = false,
    deleteAvailable: Bool = true
) -> PhotosState {
    PhotosState(
        enabled: enabled,
        consent: consent,
        photosPermission: permission,
        fullAccessReminder: reminder,
        deletionAllowed: true,
        deleteAvailable: deleteAvailable,
        newPhotoNotifications: true,
        syncing: false,
        upToDate: true,
        lastSyncMs: nil
    )
}

/// A small JPEG ImageIO decodes.
private func jpeg(width: Int = 8, height: Int = 8) -> Data {
    encoded(width: width, height: height, as: .jpeg)
}

/// A one-page PDF, which ImageIO detects but which is no image.
private func pdf() -> Data {
    let data = NSMutableData()
    var box = CGRect(x: 0, y: 0, width: 10, height: 10)
    let context = CGContext(consumer: CGDataConsumer(data: data as CFMutableData)!, mediaBox: &box, nil)!

    context.beginPDFPage(nil)
    context.endPDFPage()
    context.closePDF()
    return data as Data
}

/// A small PNG ImageIO decodes.
private func png(width: Int = 8, height: Int = 8) -> Data {
    encoded(width: width, height: height, as: .png)
}

private func encoded(width: Int, height: Int, as type: UTType) -> Data {
    let context = CGContext(
        data: nil,
        width: width,
        height: height,
        bitsPerComponent: 8,
        bytesPerRow: 0,
        space: CGColorSpaceCreateDeviceRGB(),
        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
    )!

    context.setFillColor(CGColor(red: 0.2, green: 0.5, blue: 0.8, alpha: 1))
    context.fill(CGRect(x: 0, y: 0, width: width, height: height))

    let data = NSMutableData()
    let destination = CGImageDestinationCreateWithData(data, type.identifier as CFString, 1, nil)!
    CGImageDestinationAddImage(destination, context.makeImage()!, nil)
    CGImageDestinationFinalize(destination)
    return data as Data
}

// MARK: - Fake engine

/// The engine's photos API, scripted per test. Thumbnails are read off the main thread, so its
/// state sits behind a lock.
private final class PhotosFakeApi: PhotosApiProtocol, @unchecked Sendable {
    enum Call: Equatable {
        case itemVisible(Int64), itemHidden(Int64), gridLayout([Int64]), galleryLeft
        case thumbnail(Int64), fullImage(Int64), contextMenuOpened(Int64)
        case openItem(Int64), viewerSelection(Int64), viewerItemShown(Int64), viewerClosed
        case fullImageForAction(Int64, PhotoAction)
        case prepareDelete(Int64), delete(Int64), requestDeletePermission
        case requestPhotoAccess, requestFullAccess, dismissFullAccessReminder, setConsent(Bool)
    }

    struct Script {
        var state = photosState()
        var gallery: [Photo] = []
        var thumbnails: [Int64: Data] = [:]
        var fullImages: [Int64: Data] = [:]
        /// Save-as names; by default the photo's name `Photo<id>` with `.jpg`.
        var names: [Int64: PhotoFileName] = [:]
        /// What `fullImageForAction` returns; absent means the image did not arrive in time.
        var actionImages: [Int64: Data] = [:]
        var opens = true
        var deleteCheck = PhotoDeleteCheck.confirm
        var outcome = RequestOutcome.success
        /// Thrown by the state and gallery reads.
        var failure: FeatureApiError?
        var calls: [Call] = []
        /// While set, `fullImageForAction` waits for `releaseActions()`: the image is on its way.
        var holdsActions = false
        var heldActions: [CheckedContinuation<Void, Never>] = []
        /// `fullImage` reads made on the main thread; they belong off it.
        var fullImageReadsOnMainThread = 0
    }

    private let lock = NSLock()
    private var script = Script()

    @discardableResult
    func update<T>(_ body: (inout Script) throws -> T) rethrows -> T {
        lock.lock()
        defer { lock.unlock() }
        return try body(&script)
    }

    var calls: [Call] { update { $0.calls } }

    var heldActionCount: Int { update { $0.heldActions.count } }

    var fullImageReadsOnMainThread: Int { update { $0.fullImageReadsOnMainThread } }

    func holdActions() {
        update { $0.holdsActions = true }
    }

    /// The held images arrive.
    func releaseActions() {
        let held = update { script in
            script.holdsActions = false
            defer { script.heldActions = [] }
            return script.heldActions
        }

        for continuation in held {
            continuation.resume()
        }
    }

    /// The calls that report what Quick Look does.
    var viewerCalls: [Call] {
        calls.filter {
            switch $0 {
            case .openItem, .viewerSelection, .viewerItemShown, .viewerClosed: true
            default: false
            }
        }
    }

    private func record(_ call: Call) {
        update { $0.calls.append(call) }
    }

    func state() throws -> PhotosState {
        try update { script in
            if let failure = script.failure { throw failure }
            return script.state
        }
    }

    func gallery() throws -> [Photo] {
        try update { script in
            if let failure = script.failure { throw failure }
            return script.gallery
        }
    }

    func thumbnail(id: Int64) throws -> Data? {
        record(.thumbnail(id))
        return update { $0.thumbnails[id] }
    }

    func fullImage(id: Int64) throws -> Data? {
        record(.fullImage(id))
        let onMainThread = Thread.isMainThread
        return update { script in
            if onMainThread {
                script.fullImageReadsOnMainThread += 1
            }
            return script.fullImages[id]
        }
    }

    func saveAsName(id: Int64) throws -> PhotoFileName {
        update { $0.names[id] } ?? PhotoFileName(stem: "Photo\(id)", extension: ".jpg")
    }

    func itemVisible(id: Int64) throws { record(.itemVisible(id)) }
    func itemHidden(id: Int64) throws { record(.itemHidden(id)) }
    func gridLayout(visible: [Int64]) throws { record(.gridLayout(visible)) }
    func galleryLeft() throws { record(.galleryLeft) }
    func contextMenuOpened(id: Int64) throws { record(.contextMenuOpened(id)) }

    func openItem(id: Int64) throws -> Bool {
        record(.openItem(id))
        return update { $0.opens }
    }

    func viewerSelection(id: Int64) throws { record(.viewerSelection(id)) }
    func viewerItemShown(id: Int64) throws { record(.viewerItemShown(id)) }
    func viewerClosed() throws { record(.viewerClosed) }

    func fullImageForAction(id: Int64, action: PhotoAction) async throws -> Data? {
        record(.fullImageForAction(id, action))

        await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
            let held = update { script in
                guard script.holdsActions else { return false }
                script.heldActions.append(continuation)
                return true
            }

            if !held {
                continuation.resume()
            }
        }

        return update { $0.actionImages[id] }
    }

    func prepareDelete(id: Int64) async throws -> PhotoDeleteCheck {
        record(.prepareDelete(id))
        return update { $0.deleteCheck }
    }

    func delete(id: Int64) async throws -> PhotoDeleteResult {
        record(.delete(id))
        return PhotoDeleteResult(outcome: .success, deleted: [id], reverted: [], failed: false)
    }

    func requestDeletePermission() async throws -> RequestOutcome {
        record(.requestDeletePermission)
        return update { $0.outcome }
    }

    func requestPhotoAccess() async throws -> RequestOutcome {
        record(.requestPhotoAccess)
        return update { $0.outcome }
    }

    func requestFullAccess() async throws -> RequestOutcome {
        record(.requestFullAccess)
        return update { $0.outcome }
    }

    func dismissFullAccessReminder() throws { record(.dismissFullAccessReminder) }
    func setConsent(on: Bool) throws { record(.setConsent(on)) }
    func setDeletionAllowed(on: Bool) throws {}
    func setNewPhotoNotifications(on: Bool) throws {}
}
