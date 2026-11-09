import AppKit
import PhoneLinkEngine
import PhoneLinkKit
import SwiftUI
import UniformTypeIdentifiers

/// The gallery: square thumbnails, cropped to fill, in the order the engine lists them. A click
/// selects one photo, Command-click adds or removes one, Shift-click selects a range and Command-A
/// selects all, as in the Finder; double-click or Space opens Quick Look on the last photo clicked.
/// The toolbar's Save, Share and Delete and the Edit menu act on the selection, the context menu on
/// the selection when its photo is part of it, and a photo drags out as a file.
struct PhotosGrid: View {
    @Environment(PhotosModel.self) private var model
    @State private var keyboard = PhotosGridKeyboard()

    var body: some View {
        VStack(spacing: 0) {
            if !model.selection.isEmpty {
                HStack(spacing: 12) {
                    Text("\(model.selection.count) selected")
                        .font(.system(size: 13, weight: .medium))
                        .foregroundStyle(.secondary)
                    Spacer()
                    saveMenu
                    Button {
                        PhotosFilePanels.share(model.selectedIDs, model: model, anchor: .pointer(in: keyboard.window))
                    } label: {
                        Label("Share", systemImage: "square.and.arrow.up")
                    }
                    .buttonStyle(.bordered)
                    .help("Share")
                    .disabled(model.selection.isEmpty)
                    if model.state?.deleteAvailable == true {
                        Button {
                            model.delete(model.deletable(model.selectedIDs))
                        } label: {
                            Label("Delete", systemImage: "trash")
                        }
                        .buttonStyle(.bordered)
                        .help("Delete from Phone")
                        .disabled(model.deletable(model.selectedIDs).isEmpty)
                    }
                }
                .padding(.horizontal, 16)
                .padding(.vertical, 8)
                .background(.thinMaterial)
                Divider()
            }

            GeometryReader { geometry in
                let columns = Self.columnCount(width: geometry.size.width)
                ScrollViewReader { scroller in
                    ScrollView {
                        VStack(spacing: 0) {
                            if model.state?.fullAccessReminder == true {
                                PhotosFullAccessNotice()
                            }
                            LazyVGrid(
                                columns: Array(repeating: GridItem(.flexible(), spacing: Self.spacing), count: columns),
                                spacing: Self.spacing
                            ) {
                                ForEach(model.items) { item in
                                    cell(item)
                                }
                            }
                        }
                    }
                    .background {
                        PhotosKeyResponder(model: model, keyboard: keyboard, columns: columns, preview: model.preview)
                    }
                    .onChange(of: model.focus) { _, focus in
                        if let focus {
                            scroller.scrollTo(focus)
                        }
                    }
                }
            }
        }
    }

    /// Save: the button saves the selection (a folder is chosen once for several photos); its menu
    /// also saves every recent photo.
    private var saveMenu: some View {
        Menu {
            Button(model.selection.count > 1 ? "Save \(model.selection.count) Photos…" : "Save Photo…") {
                PhotosFilePanels.save(model.selectedIDs, model: model, window: keyboard.window)
            }
            .disabled(model.selection.isEmpty)
            Button("Save All Photos…") {
                PhotosFilePanels.saveToFolder(model.items.map(\.id), model: model, window: keyboard.window)
            }
        } label: {
            Label("Save", systemImage: "square.and.arrow.down")
        } primaryAction: {
            if model.selection.isEmpty {
                NSSound.beep()
            } else {
                PhotosFilePanels.save(model.selectedIDs, model: model, window: keyboard.window)
            }
        }
        .help("Save the selected photos")
        .disabled(model.items.isEmpty)
    }

    /// A click as the Finder reads it: Command adds or removes the photo, Shift extends the range,
    /// and a plain click selects the photo alone. A Control-click opens the context menu, which
    /// selects the photo itself (`PhotosKeyView`).
    private func click(_ id: Int64) {
        let modifiers = NSEvent.modifierFlags
        if modifiers.contains(.control) {
            return
        } else if modifiers.contains(.command) {
            model.toggleSelection(id)
        } else if modifiers.contains(.shift) {
            model.extendSelection(to: id)
        } else {
            model.select(id)
        }
    }

    private func cell(_ item: PhotosGridItem) -> some View {
        PhotosGridCell(item: item, selected: model.selection.contains(item.id))
            .id(item.id)
            .onAppear { model.itemAppeared(item.id) }
            .onDisappear {
                model.itemDisappeared(item.id)
                // A cell removed under a resting pointer gets no hover exit.
                keyboard.pointer(over: item.id, inside: false)
            }
            .onHover { keyboard.pointer(over: item.id, inside: $0) }
            // The single click is simultaneous with the double click, so selecting waits for no
            // double-click interval.
            .onTapGesture(count: 2) {
                keyboard.focus()
                model.openViewer(item.id)
            }
            .simultaneousGesture(TapGesture().onEnded {
                keyboard.focus()
                click(item.id)
            })
            .contextMenu {
                PhotosContextMenu(item: item, model: model, keyboard: keyboard)
            }
            .onDrag { dragProvider(item) }
            .accessibilityAction { model.openViewer(item.id) }
    }

    /// The photo as a file promise: its full image is fetched as the drag starts and written when
    /// the drop target asks for the file.
    private func dragProvider(_ item: PhotosGridItem) -> NSItemProvider {
        let provider = NSItemProvider()
        guard let pending = model.startFile(item.id, for: .dragOut) else { return provider }
        provider.suggestedName = pending.fileName

        provider.registerFileRepresentation(
            forTypeIdentifier: pending.contentType.identifier,
            fileOptions: [],
            visibility: .all
        ) { [model] completion in
            Task { @MainActor in
                if let file = await model.file(from: pending) {
                    completion(file.url, false, nil)
                } else {
                    completion(nil, false, CocoaError(.fileReadUnknown))
                }
            }
            return nil
        }

        return provider
    }

    static let spacing: CGFloat = 2

    /// The fewest columns that keep cells at most 180 pt wide: from three columns on, cells are at
    /// least 120 pt (the narrowest main window leaves room for four of about 150 pt).
    static func columnCount(width: CGFloat) -> Int {
        max(1, Int(((width + spacing) / (180 + spacing)).rounded(.up)))
    }
}

/// One photo: its thumbnail cropped square, a neutral fill until it loads, the accent ring when
/// selected, a spinner while an action waits on the phone, dimmed while it is being deleted.
struct PhotosGridCell: View {
    let item: PhotosGridItem
    let selected: Bool

    var body: some View {
        Color.clear
            .aspectRatio(1, contentMode: .fit)
            .overlay {
                if let thumbnail = item.thumbnail {
                    Image(nsImage: thumbnail)
                        .resizable()
                        .scaledToFill()
                } else {
                    Rectangle().fill(.quaternary)
                }
            }
            .clipped()
            .overlay {
                if item.isBusy {
                    ProgressView().controlSize(.small)
                }
            }
            .overlay {
                if selected {
                    Rectangle().strokeBorder(Color.accentColor, lineWidth: 3)
                }
            }
            .opacity(opacity)
            .animation(.default, value: item.photo.deleteState)
            .contentShape(Rectangle())
            .accessibilityElement()
            .accessibilityLabel(item.photo.name)
            .accessibilityAddTraits(selected ? [.isImage, .isSelected] : .isImage)
    }

    private var opacity: Double {
        switch item.photo.deleteState {
        case .none: 1
        case .deleting: 0.4
        case .deleted: 0
        }
    }
}

/// An item's context menu: most used first, Delete last and only when something can be deleted. It
/// acts on the selection when the item is part of it, otherwise on the item alone, and counts the
/// photos in its titles when there are several.
struct PhotosContextMenu: View {
    let item: PhotosGridItem
    let model: PhotosModel
    let keyboard: PhotosGridKeyboard

    var body: some View {
        let targets = model.targets(clicked: item.id)
        let deletable = model.deletable(targets)
        if targets.count == 1 {
            Button("Open") { model.open(item.id) }
            PhotosOpenWithMenu(item: item, model: model, keyboard: keyboard)
        } else {
            Button("Open \(targets.count) Photos") {
                for id in targets {
                    model.open(id)
                }
            }
        }
        Divider()
        if targets.count == 1 {
            Button("Copy") { model.copy(item.id) }
            Button("Save As…") { PhotosFilePanels.saveAs(item.id, model: model, window: keyboard.window) }
        } else {
            Button("Copy \(targets.count) Photos") { model.copy(targets) }
            Button("Save \(targets.count) Photos…") {
                PhotosFilePanels.saveToFolder(targets, model: model, window: keyboard.window)
            }
        }
        Button("Share…") { PhotosFilePanels.share(targets, model: model, anchor: .pointer(in: keyboard.window)) }
        if !deletable.isEmpty {
            Divider()
            Button(targets.count == 1 ? "Delete…" : "Delete \(deletable.count) Photos…", role: .destructive) {
                model.delete(deletable)
            }
        }
    }
}

/// Open With: the default app for the photo's type, the other apps that open it, and Other… to pick
/// any app. A name that is no photo's or video's lists no app (opening it is refused).
struct PhotosOpenWithMenu: View {
    let item: PhotosGridItem
    let model: PhotosModel
    let keyboard: PhotosGridKeyboard

    var body: some View {
        Menu("Open With") {
            let apps = model.openWithContentType(of: item.id).map(PhotosOpenWithApps.apps(for:)) ?? []
            ForEach(apps) { app in
                Button {
                    model.open(item.id, with: app.url)
                } label: {
                    Label {
                        Text(app.isDefault ? String(localized: "\(app.name) (default)") : app.name)
                    } icon: {
                        Image(nsImage: app.icon)
                    }
                }
                if app.isDefault && apps.count > 1 {
                    Divider()
                }
            }
            if !apps.isEmpty {
                Divider()
            }
            Button("Other…") { PhotosFilePanels.chooseApp(item.id, model: model, window: keyboard.window) }
        }
    }
}
