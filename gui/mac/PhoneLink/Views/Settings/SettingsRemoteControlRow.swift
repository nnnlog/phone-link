import PhoneLinkKit
import SwiftUI

/// The remote-control switch of General's phone section: whether the selected phone may lock, sleep,
/// restart or shut down this Mac. Disabled while it does not apply.
struct SettingsRemoteControlRow: View {
    @Environment(RemoteControlModel.self) private var model

    var body: some View {
        @Bindable var model = model
        Toggle(isOn: $model.allowsRemoteControl) {
            Text("Allow the phone to control this Mac")
            Text(model.detail)
        }
        .disabled(!model.toggleApplies)
        .onAppear { model.reload() }
        .alert(model.failure?.title ?? "", isPresented: failurePresented, presenting: model.failure) { _ in
        } message: { failure in
            Text(failure.message)
        }
    }

    private var failurePresented: Binding<Bool> {
        Binding(get: { model.failure != nil }, set: { if !$0 { model.failure = nil } })
    }
}
