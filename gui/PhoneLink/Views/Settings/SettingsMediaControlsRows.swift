import PhoneLinkKit
import SwiftUI

/// The media-controls switches of General's phone section: whether what plays on the selected phone
/// shows here (disabled while it does not apply), and whether it also shows in Now Playing and answers
/// the media keys.
struct SettingsMediaControlsRows: View {
    @Environment(MediaControlsModel.self) private var model

    var body: some View {
        @Bindable var model = model
        Toggle(isOn: $model.showsControls) {
            Text("Show what plays on the phone")
            Text(model.settingDetail)
        }
        .disabled(!model.settingApplies)
        .onAppear { model.reload() }
        .alert(model.failure?.title ?? "", isPresented: failurePresented, presenting: model.failure) { _ in
        } message: { failure in
            Text(failure.message)
        }
        Toggle(isOn: $model.usesNowPlaying) {
            Text("Use media keys and Control Center")
            Text("While your phone has media open, the keyboard's play, pause and skip keys and Control Center's Now Playing control it instead of apps on this Mac.")
        }
        .disabled(!model.showsControls)
    }

    private var failurePresented: Binding<Bool> {
        Binding(get: { model.failure != nil }, set: { if !$0 { model.failure = nil } })
    }
}
