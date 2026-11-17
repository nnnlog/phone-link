import QtQuick
import QtQuick.Layouts
import QtWebEngine
import PhoneLink

// The Microsoft sign-in page, hosted while the engine waits; the page's answers go back to the engine.
// Before the first sign-in it shows a one-time notice that this computer is registered with the account
// as a Windows device.
ColumnLayout {
    id: signIn

    readonly property bool noticeShown: {
        const page = JSON.parse(EngineBridge.query("shell.signIn", "")) || {}
        return page.kind === "login" && !ShellPreferences.didSeeDeviceRegistrationNotice
    }

    spacing: 0

    Loader {
        Layout.fillWidth: true
        Layout.fillHeight: true
        sourceComponent: signIn.noticeShown ? notice : page
    }

    Rectangle {
        Layout.fillWidth: true
        implicitHeight: 1
        color: Theme.separator
    }

    PillButton {
        Layout.alignment: Qt.AlignRight
        Layout.margins: 12
        text: L10n.t("Cancel")
        onClicked: EngineBridge.cancelSignIn(true)
    }

    Component {
        id: notice

        Item {
            id: noticePage

            readonly property var about: JSON.parse(EngineBridge.query("shell.about", "")) || {}

            ColumnLayout {
                anchors.centerIn: parent
                width: Math.min(440, parent.width - 80)
                spacing: 14

                Image {
                    Layout.alignment: Qt.AlignHCenter
                    Layout.preferredWidth: 64
                    Layout.preferredHeight: 64
                    visible: !!noticePage.about.icon
                    source: noticePage.about.icon || ""
                    sourceSize.width: 128
                    sourceSize.height: 128
                }

                Text {
                    Layout.fillWidth: true
                    text: L10n.t("Connect Your Phone")
                    color: Theme.text
                    font.pixelSize: Theme.titleSize
                    font.weight: Font.DemiBold
                    horizontalAlignment: Text.AlignHCenter
                    wrapMode: Text.Wrap
                }

                Text {
                    Layout.fillWidth: true
                    text: L10n.t("Phone Link registers this computer with your Microsoft account as a Windows device so your phone can reach it. Microsoft does not offer a way to remove that registration. You can reset it later in Settings › Advanced.")
                    color: Theme.secondaryText
                    font.pixelSize: Theme.bodySize
                    horizontalAlignment: Text.AlignHCenter
                    wrapMode: Text.Wrap
                }

                PillButton {
                    Layout.alignment: Qt.AlignHCenter
                    Layout.topMargin: 6
                    prominent: true
                    focus: true
                    text: L10n.t("Continue")
                    onClicked: ShellPreferences.didSeeDeviceRegistrationNotice = true
                }
            }
        }
    }

    Component {
        id: page

        WebEngineView {
            id: web

            profile: WebEngineProfile {
                offTheRecord: true
                httpUserAgent: EngineBridge.signInUserAgent
            }

            function run(script) {
                if (script.length === 0)
                    return
                web.runJavaScript(script, result => EngineBridge.signInScriptResult(
                    result === undefined || result === null ? "" : String(result)))
            }

            Component.onCompleted: {
                const script = WebEngine.script()
                script.name = "phonelink-bridge"
                script.sourceCode = EngineBridge.signInScript
                script.injectionPoint = WebEngineScript.DocumentCreation
                script.worldId = WebEngineScript.MainWorld
                script.runsOnSubFrames = true
                web.userScripts.insert(script)
                EngineBridge.prepareSignIn(web.profile)
                web.url = EngineBridge.signInUrl
            }

            onJavaScriptConsoleMessage: (level, message, lineNumber, sourceID) => run(EngineBridge.signInConsoleMessage(message))

            onLoadingChanged: loadingInfo => {
                if (loadingInfo.status === WebEngineView.LoadSucceededStatus)
                    run(EngineBridge.signInPageLoaded(loadingInfo.url.toString()))
                else if (loadingInfo.status === WebEngineView.LoadFailedStatus)
                    EngineBridge.perform("signIn.loadFailed", JSON.stringify({
                        code: loadingInfo.errorCode, domain: loadingInfo.errorDomain, error: loadingInfo.errorString }))
            }
        }
    }
}
