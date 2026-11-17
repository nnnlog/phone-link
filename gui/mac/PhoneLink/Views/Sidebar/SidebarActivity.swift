import PhoneLinkKit
import SwiftUI

/// The bottom of the sidebar: what is happening now, each only while it happens — a file transfer,
/// then what plays on the phone. Nothing while neither runs.
struct SidebarActivity: View {
    @Environment(FileTransferModel.self) private var files
    @Environment(MediaControlsModel.self) private var media

    var body: some View {
        let transfer = files.isSending || files.transfers.receive != nil
        let playing = media.isShown && media.selected != nil
        if transfer || playing {
            VStack(spacing: 6) {
                if transfer {
                    TransferCard()
                }
                if playing {
                    NowPlayingModule()
                }
            }
            .padding(.horizontal, SidebarMetrics.edge)
            .padding(.bottom, SidebarMetrics.edge)
        }
    }
}

/// The running transfer: its file, how much is done, a 4 pt progress bar right under the name, and
/// Stop for a send. The bar is drawn rather than a system progress view, whose own vertical padding
/// pushed the name away from it.
private struct TransferCard: View {
    @Environment(FileTransferModel.self) private var files

    var body: some View {
        HStack(spacing: 6) {
            Image(systemName: files.isSending ? "arrow.up.circle.fill" : "arrow.down.circle.fill")
                .font(.system(size: 26))
                .foregroundStyle(.tint)
                .frame(width: 26, height: 26)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 5) {
                HStack(alignment: .firstTextBaseline) {
                    Text(name)
                        .font(.subheadline.weight(.semibold))
                        .lineLimit(1)
                        .truncationMode(.middle)
                    Spacer(minLength: 4)
                    if let amount {
                        Text(amount)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .monospacedDigit()
                    }
                }
                ProgressBar(fraction: fraction)
            }
            if files.isSending {
                Button {
                    files.cancelSend()
                } label: {
                    Image(systemName: "xmark.circle.fill")
                        .font(.system(size: 14))
                        .frame(width: 28, height: 28)
                }
                .buttonStyle(.borderless)
                .foregroundStyle(.secondary)
                .help("Stop Sending")
            }
        }
        .padding(.horizontal, SidebarMetrics.content - SidebarMetrics.edge)
        .padding(.vertical, 8)
        .background(SidebarModuleFill(), in: RoundedRectangle(cornerRadius: SidebarMetrics.moduleRadius, style: .continuous))
        .accessibilityElement(children: .combine)
    }

    private var names: [String] {
        files.transfers.send?.fileNames ?? files.transfers.receive?.fileNames ?? []
    }

    private var name: String {
        switch names.count {
        case 0: files.isSending ? String(localized: "Sending to your phone") : String(localized: "Receiving from your phone")
        case 1: names[0]
        default: String(localized: "\(names.count) files")
        }
    }

    /// "12 of 20 MB" once the size is known; before that what the transfer is waiting on.
    private var amount: String? {
        if let send = files.transfers.send, send.bytesTotal > 0 {
            return Self.amount(send.bytesDone, of: send.bytesTotal)
        }
        if let receive = files.transfers.receive, receive.bytesTotal > 0 {
            return Self.amount(receive.bytesDone, of: receive.bytesTotal)
        }
        return files.isSending ? files.sendStatus : nil
    }

    private var fraction: Double? {
        if files.isSending { return files.sendFraction }
        guard let receive = files.transfers.receive, receive.bytesTotal > 0 else { return nil }
        return Double(receive.bytesDone) / Double(receive.bytesTotal)
    }

    private static func amount(_ done: UInt64, of total: UInt64) -> String {
        let formatter = ByteCountFormatter()
        formatter.countStyle = .file
        let totalText = formatter.string(fromByteCount: Int64(total))
        formatter.includesUnit = false
        return String(localized: "\(formatter.string(fromByteCount: Int64(done))) of \(totalText)", comment: "Transfer progress, e.g. 12 of 20 MB")
    }
}

/// A 4 pt progress bar; while the fraction is unknown, a sliver that marks the transfer as started.
private struct ProgressBar: View {
    let fraction: Double?

    var body: some View {
        GeometryReader { geometry in
            ZStack(alignment: .leading) {
                Capsule().fill(Color.primary.opacity(0.1))
                Capsule().fill(.tint)
                    .frame(width: max(4, geometry.size.width * (fraction ?? 0)))
            }
        }
        .frame(height: 4)
        .accessibilityValue(fraction.map { "\(Int($0 * 100)) percent" } ?? "Starting")
    }
}

/// What plays on the phone, as Control Center shows it: album art, title, artist and the app, then
/// previous, play/pause and next, play/pause the largest. With two or more media apps the app name
/// is a menu that picks the one the buttons act on. The phone gives no position or duration, so
/// there is no progress bar.
private struct NowPlayingModule: View {
    @Environment(MediaControlsModel.self) private var media

    var body: some View {
        if let session = media.selected {
            VStack(spacing: 6) {
                HStack(spacing: 12) {
                    AlbumArt(image: session.albumArt)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(session.title)
                            .font(.headline)
                            .lineLimit(1)
                        if let artist = session.artist, !artist.isEmpty {
                            Text(artist)
                                .font(.subheadline)
                                .foregroundStyle(.secondary)
                                .lineLimit(1)
                        }
                        SourceLine(media: media, session: session)
                            .padding(.top, 2)
                    }
                    Spacer(minLength: 0)
                }
                HStack(spacing: 20) {
                    Button {
                        media.previous()
                    } label: {
                        Image(systemName: "backward.fill")
                            .font(.system(size: 13))
                            .frame(width: 26, height: 26)
                    }
                    .help("Previous Track")
                    Button {
                        media.playPause()
                    } label: {
                        Image(systemName: session.playing ? "pause.fill" : "play.fill")
                            .font(.system(size: 18))
                            .frame(width: 28, height: 28)
                    }
                    .help(session.playing ? "Pause" : "Play")
                    Button {
                        media.next()
                    } label: {
                        Image(systemName: "forward.fill")
                            .font(.system(size: 13))
                            .frame(width: 26, height: 26)
                    }
                    .help("Next Track")
                }
                .buttonStyle(.borderless)
                .foregroundStyle(.primary)
                .overlay(alignment: .trailing) {
                    if media.isCommanding {
                        ProgressView()
                            .controlSize(.small)
                            .offset(x: 28)
                    }
                }
            }
            .padding(.horizontal, SidebarMetrics.content - SidebarMetrics.edge)
            .padding(.vertical, 10)
            .background(SidebarModuleFill(), in: RoundedRectangle(cornerRadius: SidebarMetrics.moduleRadius, style: .continuous))
            .accessibilityElement(children: .contain)
            .accessibilityLabel("Now playing on your phone")
        }
    }
}

/// The media app's icon and name; with two or more media apps, a menu of them.
private struct SourceLine: View {
    let media: MediaControlsModel
    let session: MediaControlsModel.Session

    var body: some View {
        let label = HStack(spacing: 4) {
            Group {
                if let icon = session.appIcon {
                    Image(nsImage: icon)
                        .renderingMode(.template)
                        .resizable()
                        .interpolation(.high)
                        .scaledToFit()
                        .foregroundStyle(.secondary)
                } else {
                    Image(systemName: "music.note")
                        .font(.system(size: 8, weight: .bold))
                        .foregroundStyle(.white)
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                        .background(Color.secondary)
                        .clipShape(RoundedRectangle(cornerRadius: 3.5, style: .continuous))
                }
            }
            .frame(width: 14, height: 14)
            Text(session.appName)
                .font(.caption)
                .foregroundStyle(.secondary)
                .lineLimit(1)
        }
        if media.offersPicker {
            Menu {
                Picker("Control", selection: Binding(get: { session.id }, set: { media.select($0) })) {
                    ForEach(media.sessions) { s in
                        Text(s.appName).tag(s.id)
                    }
                }
                .pickerStyle(.inline)
            } label: {
                label
            }
            .menuStyle(.button)
            .buttonStyle(.plain)
            .fixedSize()
            .help("Choose the app to control")
        } else {
            label
        }
    }
}

private struct AlbumArt: View {
    let image: NSImage?

    var body: some View {
        Group {
            if let image {
                Image(nsImage: image)
                    .resizable()
                    .aspectRatio(contentMode: .fill)
            } else {
                Image(systemName: "music.note")
                    .font(.title2)
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .background(.quaternary)
            }
        }
        .frame(width: 56, height: 56)
        .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
        .accessibilityLabel("Album art")
    }
}
