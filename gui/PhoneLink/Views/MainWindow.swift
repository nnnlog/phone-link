import PhoneLinkEngine
import PhoneLinkKit
import SwiftUI

/// The main window: a sidebar of the phone and its sections, then the section's own columns — Messages,
/// Notifications and Contacts are list-detail (three columns with the sidebar), Photos and Apps are grids
/// (two).
/// Until the engine runs, the window shows where the start is instead.
struct MainWindow: View {
    @Environment(EngineController.self) private var controller
    @Environment(AppNavigator.self) private var navigator

    var body: some View {
        @Bindable var navigator = navigator
        Group {
            if controller.phase == .running && !controller.phoneUnlinked {
                switch navigator.section {
                case .messages:
                    NavigationSplitView {
                        Sidebar(selection: $navigator.section)
                    } content: {
                        ConversationListColumn()
                    } detail: {
                        ConversationDetailColumn()
                    }
                case .notifications:
                    NavigationSplitView {
                        Sidebar(selection: $navigator.section)
                    } content: {
                        NotificationListColumn()
                    } detail: {
                        NotificationDetailColumn()
                    }
                case .contacts:
                    NavigationSplitView {
                        Sidebar(selection: $navigator.section)
                    } content: {
                        ContactListColumn()
                    } detail: {
                        ContactDetailColumn()
                    }
                case .photos:
                    NavigationSplitView {
                        Sidebar(selection: $navigator.section)
                    } detail: {
                        PhotoGridColumn()
                    }
                case .apps:
                    NavigationSplitView {
                        Sidebar(selection: $navigator.section)
                    } detail: {
                        AppsColumn()
                    }
                }
            } else {
                EngineStateView()
            }
        }
        // Room for the sidebar, a list and a readable conversation; and for the phone area, four
        // sections, a transfer and what plays, all at once.
        .frame(minWidth: 1000, minHeight: 640)
    }
}

/// The sidebar: the phone (its status and controls) on top, whatever section is selected; the
/// sections below; and at the bottom what is happening now (a transfer, what plays on the phone).
/// The phone's wallpaper shows behind it while the user keeps it on.
private struct Sidebar: View {
    @Binding var selection: AppSection
    @Environment(MessagesModel.self) private var messages
    @Environment(NotificationsModel.self) private var notifications
    @Environment(PhoneAppsModel.self) private var phoneApps
    @Environment(ContactsModel.self) private var contacts

    /// The sections listed: Contacts only for a phone that syncs its contacts, Apps only while the
    /// phone opens its apps on this Mac.
    private var sections: [AppSection] {
        AppSection.allCases.filter { section in
            switch section {
            case .contacts: contacts.isViewVisible
            case .apps: phoneApps.isViewVisible
            case .notifications, .messages, .photos: true
            }
        }
    }

    var body: some View {
        List(selection: Binding<AppSection?>(get: { selection }, set: { if let s = $0 { selection = s } })) {
            ForEach(sections) { section in
                Label(section.title, systemImage: section.symbol)
                    .badge(badge(for: section))
                    .tag(section)
            }
        }
        .safeAreaInset(edge: .top, spacing: 0) {
            PhoneArea()
        }
        .safeAreaInset(edge: .bottom, spacing: 0) {
            SidebarActivity()
        }
        .phoneWallpaperBackground()
        .navigationSplitViewColumnWidth(min: 320, ideal: 320, max: 380)
        .onChange(of: phoneApps.isViewVisible) { _, visible in
            if !visible && selection == .apps {
                selection = .messages
            }
        }
        .onChange(of: contacts.isViewVisible) { _, visible in
            if !visible && selection == .contacts {
                selection = .messages
            }
        }
    }

    private func badge(for section: AppSection) -> Int {
        switch section {
        case .messages: messages.unreadCount
        case .notifications: notifications.badgeCount
        case .contacts, .photos, .apps: 0
        }
    }
}

/// What the window shows while the engine is not running.
private struct EngineStateView: View {
    @Environment(EngineController.self) private var controller
    @Environment(AppNavigator.self) private var navigator

    var body: some View {
        switch controller.phase {
        case .idle, .starting:
            ProgressView("Connecting to your phone…")
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        case .webSignIn:
            ContentUnavailableView {
                Label("Sign In to Continue", systemImage: "person.crop.circle")
            } description: {
                Text("Finish signing in to your Microsoft account in the sign-in window.")
            } actions: {
                Button("Show Sign-In") { navigator.showSignIn() }
                    .buttonStyle(.borderedProminent)
            }
        case .signedOut:
            ContentUnavailableView {
                Label("Signed Out", systemImage: "person.crop.circle")
            } description: {
                Text("Sign in with the Microsoft account your phone uses with Link to Windows.")
            } actions: {
                Button("Sign In") { controller.start() }
                    .buttonStyle(.borderedProminent)
            }
        case .selectingPhone:
            ContentUnavailableView {
                Label("Choose Your Phone", systemImage: "iphone")
            } description: {
                PhonePickerView()
                    .frame(maxWidth: 360)
            }
        case let .failed(message):
            ContentUnavailableView {
                Label("Phone Link Couldn't Connect", systemImage: "exclamationmark.triangle")
            } description: {
                Text(message)
            } actions: {
                Button("Try Again") { controller.start() }
                    .buttonStyle(.borderedProminent)
            }
        case .accountRemoved:
            ContentUnavailableView {
                Label("Sign In Again", systemImage: "person.crop.circle.badge.exclamationmark")
            } description: {
                Text("This Mac was removed from your Microsoft account's devices.")
            } actions: {
                Button("Sign In") { controller.start() }
                    .buttonStyle(.borderedProminent)
            }
        case .running:
            ContentUnavailableView {
                Label("Phone Not Linked", systemImage: "iphone.slash")
            } description: {
                Text("Link your phone to this Microsoft account with Link to Windows on the phone, then try again.")
            } actions: {
                Button("Try Again") { controller.start() }
                    .buttonStyle(.borderedProminent)
            }
        }
    }
}
