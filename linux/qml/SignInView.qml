import QtQuick
import QtQuick.Layouts
import QtWebEngine
import PhoneLink

ColumnLayout {
    spacing: 0

    WebEngineView {
        id: web
        Layout.fillWidth: true
        Layout.fillHeight: true

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
            web.url = EngineBridge.signInUrl
        }

        onJavaScriptConsoleMessage: (level, message, lineNumber, sourceID) => run(EngineBridge.signInConsoleMessage(message))

        onLoadingChanged: loadingInfo => {
            if (loadingInfo.status === WebEngineView.LoadSucceededStatus)
                run(EngineBridge.signInPageLoaded(loadingInfo.url.toString()))
        }
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
}
