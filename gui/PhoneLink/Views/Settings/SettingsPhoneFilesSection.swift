import PhoneLinkKit
import SwiftUI

/// The phone's storage in the Finder: the switch, the permission the phone needs, and the notice that
/// deleted files go to the phone's recycle bin.
struct PhoneFilesSection: View {
    @Environment(PhoneFilesModel.self) private var model

    var body: some View {
        @Bindable var model = model
        if model.available {
            Section {
                Toggle(isOn: $model.enabled) {
                    Text("Show phone files in the Finder")
                    Text("Browse, open, copy, rename and delete your phone's files from the Finder sidebar")
                }
                if model.needsPermission {
                    LabeledContent {
                        Button(model.permissionRequested ? "Ask Again" : "Send Request to Phone") {
                            Task { await model.requestPermission() }
                        }
                    } label: {
                        Text("Your phone needs to allow access to its files")
                        if model.permissionRequested {
                            Text("Check your phone and allow Link to Windows to manage all files.")
                        }
                    }
                }
            } header: {
                Text("Phone Files")
            }
            .alert("Deleted Files Go to Your Phone's Recycle Bin", isPresented: $model.recycleNoticeShown) {
                Button("OK") { model.dismissRecycleNotice(again: true) }
                Button("Don't Show Again") { model.dismissRecycleNotice(again: false) }
            } message: {
                Text("Files you delete from your phone's folder are kept in “\(model.recycleBinName)” on your phone for 180 days.")
            }
            .alert(model.failure?.title ?? "", isPresented: failurePresented, presenting: model.failure) { _ in
            } message: { failure in
                Text(failure.message)
            }
        }
    }

    private var failurePresented: Binding<Bool> {
        Binding(get: { model.failure != nil }, set: { if !$0 { model.failure = nil } })
    }
}
