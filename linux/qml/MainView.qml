import QtQuick
import QtQuick.Layouts
import PhoneLink

RowLayout {
    id: main

    property string selectedNotificationKey
    property double selectedCallId: -1
    property int selectedContactId: -1

    signal settingsRequested()

    readonly property var notifications: JSON.parse(EngineBridge.notificationsJson)
    readonly property var conversations: JSON.parse(EngineBridge.conversationsJson)
    readonly property var calls: { EngineBridge.callsRevision; return JSON.parse(EngineBridge.query("calls", "")) || [] }
    readonly property var contacts: { EngineBridge.contactsRevision; return JSON.parse(EngineBridge.query("contacts", "")) || [] }
    readonly property var selectedNotification: notifications.find(n => n.key === selectedNotificationKey) || null
    readonly property var openConversation: conversations.find(c => c.threadId === EngineBridge.openThreadId) || null
    readonly property var selectedCall: calls.find(c => c.id === selectedCallId) || null
    readonly property var selectedCallHistory: selectedCall === null ? []
        : selectedCall.number.length === 0 ? [selectedCall]
        : calls.filter(c => c.number === selectedCall.number)
    readonly property string section: sidebar.section
    readonly property bool listDetail: ["notifications", "messages", "calls", "contacts"].includes(section)

    function showSection(name) {
        sidebar.section = name
    }

    spacing: 0

    onSectionChanged: {
        if (section === "calls") {
            EngineBridge.perform("calls.markSeen", "")
            EngineBridge.perform("calls.refreshIfEmpty", "")
        }
    }

    Sidebar {
        id: sidebar
        Layout.preferredWidth: Theme.sidebarWidth
        Layout.fillHeight: true
        badges: ({
            "notifications": main.notifications.length,
            "messages": main.conversations.filter(c => c.unread > 0).length,
            "calls": main.calls.filter(c => c.kind === "missed" && !c.read).length
        })
        onSettingsRequested: main.settingsRequested()
    }

    Rectangle {
        Layout.preferredWidth: 1
        Layout.fillHeight: true
        color: Theme.separator
    }

    NotificationList {
        Layout.preferredWidth: Theme.listWidth
        Layout.fillHeight: true
        visible: main.section === "notifications"
        notifications: main.notifications
        selectedKey: main.selectedNotificationKey
        onSelectedKeyChanged: main.selectedNotificationKey = selectedKey
    }

    ConversationList {
        Layout.preferredWidth: Theme.listWidth
        Layout.fillHeight: true
        visible: main.section === "messages"
        conversations: main.conversations
    }

    CallList {
        Layout.preferredWidth: Theme.listWidth
        Layout.fillHeight: true
        visible: main.section === "calls"
        calls: main.calls
        selectedId: main.selectedCallId
        onSelectedIdChanged: main.selectedCallId = selectedId
    }

    ContactList {
        Layout.preferredWidth: Theme.listWidth
        Layout.fillHeight: true
        visible: main.section === "contacts"
        contacts: main.contacts
        selectedId: main.selectedContactId
        onSelectedIdChanged: main.selectedContactId = selectedId
    }

    Rectangle {
        Layout.preferredWidth: 1
        Layout.fillHeight: true
        visible: main.listDetail
        color: Theme.separator
    }

    NotificationDetail {
        Layout.fillWidth: true
        Layout.fillHeight: true
        visible: main.section === "notifications"
        notification: main.selectedNotification
    }

    ConversationDetail {
        Layout.fillWidth: true
        Layout.fillHeight: true
        visible: main.section === "messages"
        conversation: main.openConversation
    }

    CallDetail {
        Layout.fillWidth: true
        Layout.fillHeight: true
        visible: main.section === "calls"
        calls: main.selectedCallHistory
    }

    ContactDetail {
        Layout.fillWidth: true
        Layout.fillHeight: true
        visible: main.section === "contacts"
        contactId: main.selectedContactId
    }

    PhotoGrid {
        Layout.fillWidth: true
        Layout.fillHeight: true
        visible: main.section === "photos"
    }

    AppGrid {
        Layout.fillWidth: true
        Layout.fillHeight: true
        visible: main.section === "apps"
    }
}
