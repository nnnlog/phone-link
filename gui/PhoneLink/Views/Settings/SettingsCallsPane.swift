import PhoneLinkEngine
import PhoneLinkKit
import SwiftUI

/// Calls: the phone's call history on this Mac, and placing and answering the phone's calls from this
/// Mac over Bluetooth, with the calls themselves on the phone.
struct SettingsCallsPane: View {
    @Environment(SettingsModel.self) private var model
    @Environment(CallControlModel.self) private var calls
    @State private var introShown = false

    /// Microsoft's page on emergency calls with Phone Link.
    private static let emergencyInfo = URL(string: "https://go.microsoft.com/fwlink/?linkid=2111247")

    var body: some View {
        @Bindable var model = model
        Form {
            if !model.isAvailable {
                SettingsUnavailableNote()
            }
            if let history = model.callingState, history.access != .unsupported {
                Section {
                    Toggle("Show call history on this Mac", isOn: $model.showsCallHistory)
                    // Only once the phone shares call history and nothing but its permission is left.
                    if history.access == .devicePermissionRequired || history.access == .granted {
                        SettingsAccessRow(
                            title: "Call history access",
                            allowed: history.access == .granted,
                            pending: model.pendingRequests.contains(.callHistoryAccess)
                        ) {
                            Task { await model.requestCallHistoryAccess() }
                        }
                    }
                }
                .disabled(!model.isAvailable)

                Section {
                    // While call history is off the switch says why it is off, under its label.
                    Toggle(isOn: controlEnabled) {
                        Text("Make and answer calls on this Mac")
                        if !history.pcConsent {
                            Text("Turn on call history to make and answer calls on this Mac.")
                        }
                    }
                    .disabled(!history.pcConsent || !calls.isOffered)
                    if calls.isEnabled && history.pcConsent {
                        LabeledContent("Bluetooth") {
                            Text(calls.statusText)
                        }
                        if let notice = calls.connectionNotice {
                            CallControlNoticeView(notice: notice)
                        }
                    }
                } footer: {
                    VStack(alignment: .leading, spacing: 8) {
                        CallsStayOnPhoneNote()
                        if let url = Self.emergencyInfo {
                            Link("Contacting emergency services using Phone Link", destination: url)
                                .font(.callout)
                        }
                    }
                }
                .disabled(!model.isAvailable)
            } else {
                Section {
                    Text("Calls need an Android phone that can make phone calls.")
                        .foregroundStyle(.secondary)
                }
            }
        }
        .settingsPaneForm()
        .onAppear { model.reload() }
        .callControlIntro(isPresented: $introShown)
    }

    /// Switching on explains first, the first time on this Mac.
    private var controlEnabled: Binding<Bool> {
        Binding {
            calls.isEnabled
        } set: { on in
            if on && calls.needsIntro {
                introShown = true
            } else {
                calls.setEnabled(on)
            }
        }
    }
}
