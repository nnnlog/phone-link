import PhoneLinkKit
import SwiftUI

/// A phone app's icon from the phone's app list, or a placeholder when the app sent none, sent bytes
/// that are no image, or is not in the list.
struct PhoneAppIcon: View {
    let packageName: String
    var size: CGFloat = 48
    @Environment(PhoneAppsModel.self) private var model

    var body: some View {
        Group {
            if let image = model.icon(for: packageName) {
                Image(nsImage: image)
                    .resizable()
                    .interpolation(.high)
                    .scaledToFit()
            } else {
                RoundedRectangle(cornerRadius: size * 0.22, style: .continuous)
                    .fill(.quaternary)
                    .overlay {
                        Image(systemName: "app.dashed")
                            .font(.system(size: size * 0.45))
                            .foregroundStyle(.secondary)
                    }
            }
        }
        .frame(width: size, height: size)
        .accessibilityHidden(true)
    }
}
