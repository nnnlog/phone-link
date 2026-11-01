import PhoneLinkEngine
import PhoneLinkKit
import SwiftUI
import WebKit

private let log = AppLog(category: "sign-in")

/// Hosts a Microsoft-account sign-in (or consent) page in a `WKWebView`: it loads the request with the
/// headers and user agent the engine gives it, injects the engine's bridge script at document start, and reports back the JSON the page posts to the
/// `phonelink` message handler. A consent page also finishes when it reaches the request's end-URL
/// prefix: with the page's `ServerData` when that carries a sign-in result, else with `{}`. The data
/// store is non-persistent, so no web session is left behind.
struct WebSignInWebView: NSViewRepresentable {
    let request: WebSignInRequest
    let onResult: (String) -> Void

    func makeCoordinator() -> Coordinator {
        Coordinator(endURLPrefix: request.endUrlPrefix, onResult: onResult)
    }

    func makeNSView(context: Context) -> WKWebView {
        let config = WKWebViewConfiguration()
        config.websiteDataStore = .nonPersistent()
        let controller = WKUserContentController()
        controller.addUserScript(
            WKUserScript(source: request.bridgeScript, injectionTime: .atDocumentStart, forMainFrameOnly: false)
        )
        controller.add(context.coordinator, name: "phonelink")
        config.userContentController = controller

        let webView = WKWebView(frame: .zero, configuration: config)
        webView.customUserAgent = request.userAgent
        webView.navigationDelegate = context.coordinator

        var urlRequest = URLRequest(url: URL(string: request.url) ?? URL(string: "https://login.live.com")!)
        for (name, value) in request.headers {
            urlRequest.setValue(value, forHTTPHeaderField: name)
        }
        webView.load(urlRequest)
        return webView
    }

    func updateNSView(_ nsView: WKWebView, context: Context) {}

    static func dismantleNSView(_ nsView: WKWebView, coordinator: Coordinator) {
        nsView.configuration.userContentController.removeScriptMessageHandler(forName: "phonelink")
    }

    final class Coordinator: NSObject, WKScriptMessageHandler, WKNavigationDelegate {
        /// The capabilities CloudExperienceHost reports to the page (matches `cxh-capabilities`).
        private static let cxhCapabilities = #"{"PrivatePropertyBag":1,"PasswordlessConnect":1,"PreferAssociate":1,"ChromelessUI":0}"#

        private let endURLPrefix: String?
        private let onResult: (String) -> Void
        private var done = false

        init(endURLPrefix: String?, onResult: @escaping (String) -> Void) {
            self.endURLPrefix = endURLPrefix
            self.onResult = onResult
        }

        /// Every `window.external.notify` the page makes. CloudExperienceHost's page sends two kinds:
        /// a `CloudExperienceHost.getContext` request the host answers, and the signed-in result
        /// (`sDAToken`/`K`), which is the only one that completes the sign-in. Anything else is
        /// ignored, so an early status message never ends the sign-in.
        func userContentController(_ controller: WKUserContentController, didReceive message: WKScriptMessage) {
            guard !done else { return }

            let text: String
            if let s = message.body as? String {
                text = s
            } else if let obj = message.body as? [String: Any],
                      let data = try? JSONSerialization.data(withJSONObject: obj),
                      let s = String(data: data, encoding: .utf8) {
                text = s
            } else {
                return
            }

            guard let data = text.data(using: .utf8),
                  let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
            else {
                log.debug("sign-in page message ignored; it is no JSON object")
                return
            }

            if isGetContext(json) {
                answerGetContext(message.webView, context: contextString(json))
                return
            }
            if containsSignInResult(json) {
                done = true
                onResult(text)
            }
        }

        /// Once the sign-in page reaches `post.srf`, pull the page's `ServerData` (the result) out. A
        /// consent page that reaches its end URL finishes there, with its `ServerData` when that holds a
        /// result and `{}` otherwise.
        func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
            guard !done, let url = webView.url?.absoluteString else { return }
            if let end = endURLPrefix, url.hasPrefix(end) {
                webView.evaluateJavaScript("window.ServerData ? JSON.stringify(window.ServerData) : null") { [weak self] value, _ in
                    guard let self, !self.done else { return }
                    self.done = true
                    if let text = value as? String, let data = text.data(using: .utf8),
                       let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
                       self.containsSignInResult(json) {
                        self.onResult(text)
                    } else {
                        self.onResult("{}")
                    }
                }
                return
            }

            guard url.contains("post.srf") else { return }
            webView.evaluateJavaScript("window.ServerData ? window.external.notify(JSON.stringify(window.ServerData)) : 0")
        }

        func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: any Error) {
            loadFailed(error)
        }

        func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: any Error) {
            loadFailed(error)
        }

        /// A load the page replaced with another (a redirect, a new navigation) is no failure.
        private func loadFailed(_ error: any Error) {
            let ns = error as NSError
            if ns.domain == NSURLErrorDomain, ns.code == NSURLErrorCancelled {
                log.debug("sign-in page load superseded")
            } else {
                log.error("sign-in page not loaded: \(error: error)")
            }
        }

        private func isGetContext(_ json: [String: Any]) -> Bool {
            guard (json["type"] as? String) == "invoke", let value = json["value"] as? [String: Any] else { return false }
            return (value["name"] as? String) == "CloudExperienceHost.getContext"
        }

        private func contextString(_ json: [String: Any]) -> String {
            ((json["value"] as? [String: Any])?["context"] as? String) ?? ""
        }

        /// A posted result if it (or a nested object) carries both `sDAToken` and `K`.
        private func containsSignInResult(_ json: [String: Any]) -> Bool {
            if json["sDAToken"] != nil, json["K"] != nil { return true }
            for value in json.values {
                if let nested = value as? [String: Any], containsSignInResult(nested) { return true }
            }
            return false
        }

        /// Answers `CloudExperienceHost.getContext` by dispatching the host callback the page waits
        /// for, as the real CloudExperienceHost does.
        private func answerGetContext(_ webView: WKWebView?, context: String) {
            let callback: [String: Any] = [
                "type": "callback",
                "value": [
                    "name": "CloudExperienceHost.getContext",
                    "args": ["CloudExperienceHost", "TokenBroker", "TokenBroker", Self.cxhCapabilities],
                    "context": context,
                ],
            ]

            guard let data = try? JSONSerialization.data(withJSONObject: callback),
                  let payload = String(data: data, encoding: .utf8)
            else { return }

            let escaped = payload.replacingOccurrences(of: "\\", with: "\\\\").replacingOccurrences(of: "\"", with: "\\\"")
            webView?.evaluateJavaScript(
                "if(window[\"CloudExperienceHost.Bridge.dispatchMessage\"]) window[\"CloudExperienceHost.Bridge.dispatchMessage\"](\"\(escaped)\")"
            )
        }
    }
}
