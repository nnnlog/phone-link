import PhoneLinkKit
import SwiftUI

/// The phone drawn small with its wallpaper as the screen — a phone outline when there is none.
struct PhoneWallpaperThumbnail: View {
    let image: NSImage?

    private static let size = CGSize(width: 36, height: 66)

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: 8, style: .continuous)
        ZStack {
            if let image {
                Image(nsImage: image)
                    .resizable()
                    .scaledToFill()
            } else {
                Image(systemName: "iphone")
                    .font(.system(size: 32, weight: .light))
                    .foregroundStyle(.secondary)
            }
        }
        .frame(width: Self.size.width, height: Self.size.height)
        .clipShape(shape)
        .overlay {
            if image != nil {
                shape.strokeBorder(
                    LinearGradient(
                        colors: [.white.opacity(0.6), .primary.opacity(0.25)],
                        startPoint: .top, endPoint: .bottom
                    ),
                    lineWidth: 1
                )
            }
        }
        .accessibilityLabel(image == nil ? "Phone" : "Phone wallpaper")
    }
}

/// The phone's wallpaper behind a sidebar, blurred, while the user keeps it on.
struct PhoneWallpaperBackground: ViewModifier {
    @Environment(WallpaperModel.self) private var wallpaper

    func body(content: Content) -> some View {
        if wallpaper.showsBackground, let image = wallpaper.image {
            content
                .scrollContentBackground(.hidden)
                .background {
                    Image(nsImage: image)
                        .resizable()
                        .scaledToFill()
                        .blur(radius: 14, opaque: true)
                        // Muted so the sidebar stays calm beside the content: less saturated and
                        // fainter than the picture.
                        .saturation(0.7)
                        .opacity(0.3)
                        .ignoresSafeArea()
                        .accessibilityHidden(true)
                }
        } else {
            content
        }
    }
}

extension View {
    func phoneWallpaperBackground() -> some View {
        modifier(PhoneWallpaperBackground())
    }
}
