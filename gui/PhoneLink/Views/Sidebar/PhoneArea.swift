import AppKit
import PhoneLinkKit
import SwiftUI
import UniformTypeIdentifiers

/// The sidebar's grid: containers sit on the selection highlight's edges (10 pt in from the sidebar),
/// content on the row icons' column (19 pt in), so the phone area lines up with the section list.
enum SidebarMetrics {
    static let edge: CGFloat = 10
    static let content: CGFloat = 19
    /// The leading picture of a row (the phone, album art), so the text after it starts on one line.
    static let leadingColumn: CGFloat = 28
    static let moduleRadius: CGFloat = 12
}

/// The top of the sidebar, shown whatever section is selected: the phone and its status, why it is
/// not connected (and Reconnect), and its controls as tiles that name each control and its state.
/// Files dropped on it are sent to the phone.
struct PhoneArea: View {
    @Environment(EngineController.self) private var controller
    @Environment(FileTransferModel.self) private var files
    @Environment(AppNavigator.self) private var navigator
    @State private var dropTargeted = false

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            VStack(alignment: .leading, spacing: 14) {
                PhoneHeader()
                    .padding(.horizontal, SidebarMetrics.content - SidebarMetrics.edge)
                if showsConnectionNotice {
                    ConnectionNotice()
                }
                PhoneControlTiles()
            }
            .padding(.top, 6)
            .padding(.bottom, 12)
            Divider()
        }
        .padding(.bottom, 8)
        .padding(.horizontal, SidebarMetrics.edge)
        .background {
            // The frosted-glass panel the phone area sits on: translucent, with the wallpaper's
            // color behind it showing through.
            RoundedRectangle(cornerRadius: SidebarMetrics.moduleRadius, style: .continuous)
                .fill(.ultraThinMaterial)
        }
        .background {
            RoundedRectangle(cornerRadius: SidebarMetrics.moduleRadius, style: .continuous)
                .strokeBorder(.tint, lineWidth: 2)
                .opacity(dropTargeted ? 1 : 0)
                .padding(.horizontal, SidebarMetrics.edge / 2)
        }
        .onDrop(of: [.fileURL], isTargeted: $dropTargeted) { providers in
            guard files.canSend else { return false }
            Task { await send(providers) }
            return true
        }
    }

    private var showsConnectionNotice: Bool {
        controller.phone != nil && !controller.connected && !controller.allFeaturesDisabled
    }

    /// Sends the dropped files from the send window, which shows the progress and the outcome.
    private func send(_ providers: [NSItemProvider]) async {
        var urls: [URL] = []
        for provider in providers {
            if let url = try? await provider.loadItem(forTypeIdentifier: UTType.fileURL.identifier) as? URL {
                urls.append(url)
            } else if let data = try? await provider.loadItem(forTypeIdentifier: UTType.fileURL.identifier) as? Data,
                      let url = URL(dataRepresentation: data, relativeTo: nil) {
                urls.append(url)
            }
        }

        guard !urls.isEmpty else { return }
        navigator.showSendFiles()
        files.sendFromWindow(urls)
    }
}

/// The phone's picture and name, and under the name its status bar — or, while it is not connected,
/// what is happening instead. The name opens a menu: the account's other phones, pairing one, and
/// the phone's storage in the Finder. The picture opens the phone's screen.
private struct PhoneHeader: View {
    @Environment(EngineController.self) private var controller
    @Environment(WallpaperModel.self) private var wallpaper
    @Environment(ScreenMirroringModel.self) private var mirroring
    @Environment(DeviceStatusModel.self) private var deviceStatus

    var body: some View {
        HStack(spacing: 10) {
            Button {
                NSApp.activate()
                mirroring.openPhoneScreen()
            } label: {
                PhoneWallpaperThumbnail(image: wallpaper.image)
            }
            .buttonStyle(.plain)
            .disabled(!mirroring.isOffered)
            .help(mirroring.isOffered ? Text("Open Phone Screen") : Text(verbatim: ""))
            .accessibilityLabel("Open Phone Screen")
            VStack(alignment: .leading, spacing: 4) {
                PhoneNameMenu()
                if controller.connected, let summary = deviceStatus.summary {
                    StatusLine(summary: summary)
                } else {
                    ConnectionLine()
                }
            }
            Spacer(minLength: 0)
        }
    }
}

/// The phone's name as a menu.
private struct PhoneNameMenu: View {
    @Environment(EngineController.self) private var controller
    @Environment(AppNavigator.self) private var navigator
    @Environment(PhoneFilesModel.self) private var phoneFiles

    var body: some View {
        Menu {
            if phoneFiles.enabled {
                Button("Show in Finder") { Task { await phoneFiles.showInFinder() } }
                if let storage = phoneFiles.storageStatus {
                    Text(storage)
                }
                Divider()
            }
            if controller.phones.count > 1 {
                Picker("Phone", selection: Binding(
                    get: { controller.phone?.id ?? "" },
                    set: { controller.selectPhone($0) }
                )) {
                    ForEach(controller.phones, id: \.id) { phone in
                        Text(phone.name).tag(phone.id)
                    }
                }
                .pickerStyle(.inline)
                Divider()
            }
            Button("Pair a New Phone…") { navigator.showPairing() }
        } label: {
            HStack(spacing: 4) {
                Text(controller.phone?.name ?? String(localized: "Phone"))
                    .font(.title3.weight(.semibold))
                    .lineLimit(1)
                Image(systemName: "chevron.down")
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(.secondary)
                    .accessibilityHidden(true)
            }
        }
        .menuStyle(.button)
        .buttonStyle(.plain)
        .menuIndicator(.hidden)
        .fixedSize()
    }
}

/// The phone's status bar, in its own order: Wi-Fi, cellular, then the battery's percentage and
/// glyph. The gaps are set per glyph, because SF Symbols carry uneven side bearings: a uniform
/// spacing shows uneven gaps. Seen: 8 pt between items, 3 pt between the percentage and its glyph;
/// the glyphs sit on the percentage's baseline.
private struct StatusLine: View {
    let summary: PhoneStatusSummary

    var body: some View {
        HStack(spacing: 0) {
            if let wifi = summary.wifi {
                glyph(wifi)
                    .offset(y: -0.5)
                    .padding(.trailing, 4.5)
            }
            if let cellular = summary.cellular {
                glyph(cellular)
                    .offset(y: -1)
                    .padding(.trailing, 5)
            }
            if let percent = summary.batteryPercent {
                Text("\(percent)%")
                    .monospacedDigit()
                    .accessibilityHidden(true)
            }
            if let battery = summary.battery {
                glyph(battery)
            }
        }
        .font(.subheadline)
        .imageScale(.medium)
        .foregroundStyle(.secondary)
    }

    private func glyph(_ glyph: PhoneStatusSummary.Glyph) -> some View {
        Group {
            if let value = glyph.variableValue {
                Image(systemName: glyph.symbol, variableValue: value)
            } else {
                Image(systemName: glyph.symbol)
            }
        }
        .accessibilityLabel(glyph.label)
    }
}

/// What the status line says while the phone is not connected.
private struct ConnectionLine: View {
    @Environment(EngineController.self) private var controller

    var body: some View {
        HStack(spacing: 5) {
            if controller.allFeaturesDisabled {
                Text("Link to Windows is turned off")
            } else if controller.phone == nil {
                ProgressView().controlSize(.mini)
                Text("Connecting…")
            } else {
                Image(systemName: "exclamationmark.triangle.fill")
                    .foregroundStyle(.orange)
                    .accessibilityHidden(true)
                Text("Not connected")
            }
        }
        .font(.subheadline)
        .foregroundStyle(.secondary)
    }
}

/// While the phone cannot be reached: the reason, and Reconnect. The header already says it is not
/// connected, so this row gives only the reason.
private struct ConnectionNotice: View {
    @Environment(EngineController.self) private var controller

    var body: some View {
        HStack(spacing: 8) {
            Text(controller.connectFailure ?? String(localized: "Your phone can't be reached right now."))
                .font(.subheadline)
                .foregroundStyle(.secondary)
                .frame(maxWidth: .infinity, alignment: .leading)
            Button("Reconnect") { controller.refresh() }
                .controlSize(.small)
        }
        .padding(.horizontal, SidebarMetrics.content - SidebarMetrics.edge)
        .padding(.vertical, 8)
        .background(SidebarModuleFill(), in: RoundedRectangle(cornerRadius: SidebarMetrics.moduleRadius, style: .continuous))
    }
}

/// The phone's controls, two per row, each only while the phone offers it: Do Not Disturb, Ringer,
/// Hotspot, Find Phone, Send Files, Phone Screen. While the phone is not connected its modes are
/// unknown, so the tiles show names only and cannot be clicked.
private struct PhoneControlTiles: View {
    @Environment(EngineController.self) private var controller
    @Environment(DeviceStatusModel.self) private var deviceStatus
    @Environment(HotspotModel.self) private var hotspot
    @Environment(ScreenMirroringModel.self) private var mirroring
    @Environment(FileTransferModel.self) private var files
    @Environment(AppNavigator.self) private var navigator

    var body: some View {
        let tiles = self.tiles
        if !tiles.isEmpty {
            let columns = [GridItem(.flexible(), spacing: 6), GridItem(.flexible(), spacing: 6)]
            LazyVGrid(columns: columns, spacing: 6) {
                ForEach(tiles) { tile in
                    PhoneControlTileView(tile: tile, centered: tile.id == .phoneScreen) { perform(tile.id) }
                        .gridCellColumns(tile.id == .phoneScreen ? 2 : 1)
                }
            }
        }
    }

    private var tiles: [PhoneControlTile] {
        let all: [PhoneControlTile?] = [
            deviceStatus.doNotDisturbTile,
            deviceStatus.ringerTile,
            hotspot.tile,
            deviceStatus.findPhoneTile,
            files.canSend ? PhoneControlTile(.sendFiles, String(localized: "Send Files"), symbol: "arrow.up.doc") : nil,
            mirroring.isOffered ? PhoneControlTile(.phoneScreen, String(localized: "Phone Screen"), symbol: "apps.iphone") : nil,
        ]

        return all.compactMap { tile in
            guard let tile else { return nil }
            guard controller.connected else {
                return PhoneControlTile(tile.id, tile.title, symbol: tile.symbol, isEnabled: false)
            }
            return tile
        }
    }

    private func perform(_ kind: PhoneControlTile.Kind) {
        switch kind {
        case .doNotDisturb: deviceStatus.toggleDoNotDisturb()
        case .ringer: deviceStatus.cycleRingerMode()
        case .findPhone: deviceStatus.findPhone()
        case .hotspot: hotspot.toggle()
        case .sendFiles: navigator.showSendFiles()
        case .phoneScreen:
            NSApp.activate()
            mirroring.openPhoneScreen()
        }
    }
}

/// One tile: a 26 pt circle with the symbol (filled with the accent color while on), the control's
/// name and its state. Every symbol is drawn at one point size, as SF Symbols are optically matched
/// at a size; 11 pt leaves Ringer a margin inside the circle. Find Phone's waves make its symbol
/// wider than the rest, so it is drawn smaller to keep it inside the circle.
private struct PhoneControlTileView: View {
    let tile: PhoneControlTile
    /// The tile spans the whole row and its content is centered, for the mirroring tile.
    let centered: Bool
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 6) {
                if centered {
                    Spacer(minLength: 0)
                }
                Group {
                    if tile.isBusy {
                        ProgressView().controlSize(.mini)
                    } else {
                        Image(systemName: tile.symbol)
                            .font(.system(size: tile.id == .findPhone ? 9.5 : 11, weight: .semibold))
                            .foregroundStyle(tile.isOn ? AnyShapeStyle(.white) : AnyShapeStyle(.primary))
                    }
                }
                .frame(width: 26, height: 26)
                .background(tile.isOn ? AnyShapeStyle(.tint) : AnyShapeStyle(Color.primary.opacity(0.08)), in: Circle())
                VStack(alignment: .leading, spacing: 0) {
                    Text(tile.title)
                        .font(.callout.weight(.semibold))
                        .lineLimit(1)
                    if let state = tile.state {
                        Text(state)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .lineLimit(1)
                    }
                }
                Spacer(minLength: 0)
            }
            .padding(.horizontal, SidebarMetrics.content - SidebarMetrics.edge)
            .frame(minHeight: 44)
            .background(SidebarModuleFill(), in: RoundedRectangle(cornerRadius: SidebarMetrics.moduleRadius, style: .continuous))
            .contentShape(RoundedRectangle(cornerRadius: SidebarMetrics.moduleRadius, style: .continuous))
        }
        .buttonStyle(.plain)
        .disabled(!tile.isEnabled)
        .opacity(tile.isEnabled || tile.isBusy || tile.isOn ? 1 : 0.5)
        .accessibilityLabel(tile.title)
        .accessibilityValue(tile.state ?? "")
    }
}

/// The fill of the sidebar's modules: raised above the sidebar in both appearances — translucent
/// white in light mode, a faint white in dark mode (dark surfaces rise by getting lighter).
struct SidebarModuleFill: ShapeStyle {
    func resolve(in environment: EnvironmentValues) -> Color {
        environment.colorScheme == .dark ? Color.white.opacity(0.09) : Color.white.opacity(0.55)
    }
}
