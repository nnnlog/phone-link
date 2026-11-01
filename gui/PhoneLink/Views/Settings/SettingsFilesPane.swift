import PhoneLinkKit
import SwiftUI

/// Files: the phone's storage in the Finder, whether this phone is offered when files are sent from
/// this Mac, and where the files the phone sends are saved (shown once the phone has sent files).
struct SettingsFilesPane: View {
    @Environment(FileTransferModel.self) private var model
    @Environment(PhoneFilesModel.self) private var phoneFiles
    @State private var choosingFolder = false

    var body: some View {
        @Bindable var model = model
        Form {
            if model.settings == nil {
                SettingsUnavailableNote()
            }
            PhoneFilesSection()
            if model.showsShareToggle {
                Section {
                    Toggle(isOn: $model.shareEnabled) {
                        Text("Send files to this phone")
                        Text("Offer this phone when you send files from this Mac")
                    }
                }
            }
            if model.showsFolderSetting, let folder = model.receivedFilesFolder {
                Section {
                    LabeledContent("Save received files to") {
                        Text(folder.path(percentEncoded: false).replacingOccurrences(of: NSHomeDirectory(), with: "~"))
                            .lineLimit(1)
                            .truncationMode(.middle)
                    }
                    HStack {
                        Spacer()
                        Button("Open Folder") { model.openReceivedFilesFolder() }
                        Button("Change…") { choosingFolder = true }
                    }
                }
            } else if model.settings != nil {
                Section {
                    Text("Files your phone shares with this Mac are saved to Downloads › Mobile Devices.")
                        .foregroundStyle(.secondary)
                }
            }
        }
        .settingsPaneForm()
        .onAppear {
            model.reload()
            phoneFiles.reload()
        }
        .fileImporter(isPresented: $choosingFolder, allowedContentTypes: [.folder]) { result in
            if case let .success(folder) = result {
                model.changeReceivedFilesFolder(to: folder)
            }
        }
        .alert(model.failure?.title ?? "", isPresented: failurePresented, presenting: model.failure) { _ in
        } message: { failure in
            Text(failure.message)
        }
    }

    private var failurePresented: Binding<Bool> {
        Binding(get: { model.failure != nil }, set: { if !$0 { model.failure = nil } })
    }
}
