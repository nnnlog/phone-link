#pragma once

#include <QtCore/QByteArray>
#include <QtCore/QCoreApplication>
#include <QtCore/QList>
#include <QtCore/QObject>
#include <QtCore/QPair>
#include <QtCore/QPointer>
#include <QtCore/QThread>
#include <QtCore/QUrl>
#include <QtGui/QGuiApplication>
#include <QtGui/QScreen>
#include <QtGui/QWindow>
#include <QtWebEngineCore/QWebEngineUrlRequestInfo>
#include <QtWebEngineCore/QWebEngineUrlRequestInterceptor>
#include <QtWebEngineQuick/QQuickWebEngineProfile>

#include <algorithm>
#include <cmath>
#include <cstdint>
#include <mutex>
#include <utility>
#include <vector>

#include "rust/cxx.h"

namespace phonelink {

// Adds the engine's headers to the first main-frame request for the sign-in page, the address the engine
// gave with them; later navigations (redirects, the page's own loads) go out unchanged.
class SignInHeaders final : public QWebEngineUrlRequestInterceptor {
public:
    SignInHeaders(QUrl url, QList<QPair<QByteArray, QByteArray>> headers, QObject* parent)
        : QWebEngineUrlRequestInterceptor(parent)
        , m_url(std::move(url))
        , m_headers(std::move(headers))
    {
    }

    void interceptRequest(QWebEngineUrlRequestInfo& info) override
    {
        if (m_sent || info.resourceType() != QWebEngineUrlRequestInfo::ResourceTypeMainFrame
            || info.requestUrl() != m_url) {
            return;
        }
        m_sent = true;
        for (const auto& header : m_headers) {
            info.setHttpHeader(header.first, header.second);
        }
    }

private:
    QUrl m_url;
    QList<QPair<QByteArray, QByteArray>> m_headers;
    bool m_sent = false;
};

// Installs the sign-in headers on the WebEngineProfile the sign-in view loads with; false when `profile`
// is not one.
inline bool installSignInHeaders(QObject* profile, rust::Str url, const rust::Vec<rust::String>& names,
    const rust::Vec<rust::String>& values)
{
    auto* quick = qobject_cast<QQuickWebEngineProfile*>(profile);
    if (quick == nullptr || names.size() != values.size()) {
        return false;
    }

    QList<QPair<QByteArray, QByteArray>> headers;
    for (size_t index = 0; index < names.size(); ++index) {
        headers.append({ QByteArray(names[index].data(), static_cast<qsizetype>(names[index].size())),
            QByteArray(values[index].data(), static_cast<qsizetype>(values[index].size())) });
    }
    const QUrl target(QString::fromUtf8(url.data(), static_cast<qsizetype>(url.size())));
    quick->setUrlRequestInterceptor(new SignInHeaders(target, std::move(headers), quick));
    return true;
}

// Hands `token`, a notification click's activation token, to the next window activation: Qt's Wayland
// platform reads XDG_ACTIVATION_TOKEN in QWindow::requestActivate() and unsets it there, so that the
// compositor brings the window forward. It is set as KDE's KWindowSystem sets it, with qputenv, which
// takes the lock Qt's own environment functions take. The remaining risk: a getenv outside Qt on another
// thread (the C library, other libraries, Rust's std::env with its own lock) is not serialized with
// this write, and POSIX leaves setenv unsafe beside any concurrent getenv. It runs on the GUI thread only,
// right before requestActivate(), once per notification click.
inline void setActivationToken(rust::Str token)
{
    qputenv("XDG_ACTIVATION_TOKEN", QByteArray(token.data(), static_cast<qsizetype>(token.size())));
}

// The primary screen as the phone is told about it, in device pixels: its available area less a framed
// window's title bar, the dpi (96 × the scale), and its whole size.
//
// Only the GUI thread reads the screen: a call there reads it, and from the first such call on it is read
// again whenever another screen becomes the primary one or the primary screen's geometry, available
// geometry or scale changes. A call on another thread (the engine's, configuring the phone) gets what was
// read last. Empty before a screen was read; without a screen the last values stay.
inline rust::Vec<int32_t> primaryDisplayGeometry()
{
    static std::mutex lock;
    static std::vector<int32_t> lastRead;
    const auto lastValues = [] {
        const std::lock_guard<std::mutex> guard(lock);
        rust::Vec<int32_t> geometry;
        for (const int32_t value : lastRead) {
            geometry.push_back(value);
        }
        return geometry;
    };

    QCoreApplication* application = QCoreApplication::instance();
    if (application == nullptr || QThread::currentThread() != application->thread()) {
        return lastValues();
    }

    // The connections that read the screen again: made once for the primary screen changing, and for
    // each screen while it is the primary one.
    static bool followingPrimary = false;
    static QPointer<QScreen> followed;
    static QList<QMetaObject::Connection> screenChanges;
    const auto readAgain = [] { primaryDisplayGeometry(); };
    if (!followingPrimary) {
        followingPrimary = true;
        QObject::connect(qGuiApp, &QGuiApplication::primaryScreenChanged, application, readAgain);
    }
    QScreen* screen = QGuiApplication::primaryScreen();
    if (screen != followed) {
        for (const QMetaObject::Connection& connection : std::as_const(screenChanges)) {
            QObject::disconnect(connection);
        }
        screenChanges.clear();
        followed = screen;
        if (screen != nullptr) {
            screenChanges = {
                QObject::connect(screen, &QScreen::geometryChanged, application, readAgain),
                QObject::connect(screen, &QScreen::availableGeometryChanged, application, readAgain),
                QObject::connect(screen, &QScreen::logicalDotsPerInchChanged, application, readAgain),
                QObject::connect(screen, &QScreen::physicalDotsPerInchChanged, application, readAgain),
            };
        }
    }
    if (screen == nullptr) {
        return lastValues();
    }

    int titleBar = 0;
    for (const QWindow* window : QGuiApplication::topLevelWindows()) {
        if (window->isVisible() && window->frameMargins().top() > 0) {
            titleBar = window->frameMargins().top();
            break;
        }
    }

    const qreal ratio = screen->devicePixelRatio();
    const QRect available = screen->availableGeometry();
    const auto pixels = [ratio](qreal points) {
        return static_cast<int32_t>(std::floor(std::max<qreal>(0, points) * ratio));
    };
    {
        const std::lock_guard<std::mutex> guard(lock);
        lastRead = {
            pixels(available.width()),
            pixels(available.height() - titleBar),
            static_cast<int32_t>(std::lround(96 * ratio)),
            pixels(screen->size().width()),
            pixels(screen->size().height()),
        };
    }
    return lastValues();
}

}
