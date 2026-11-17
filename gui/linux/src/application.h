#pragma once

#include <QtGui/QGuiApplication>
#include <QtGui/QIcon>
#include <QtWebEngineQuick/qtwebenginequickglobal.h>
#include <QtWidgets/QApplication>

#include <memory>
#include <string>
#include <vector>

#include "rust/cxx.h"

namespace phonelink {

namespace detail {

    // The application object. A function-local static: its destructor still runs among the program's
    // static destructors, so destroyApplication() empties it before main() returns.
    inline std::unique_ptr<QApplication>& application()
    {
        static std::unique_ptr<QApplication> application;
        return application;
    }

}

// Qt's tray on KDE builds its menu as a QMenu, which needs a QApplication rather than the
// QGuiApplication that cxx-qt-lib offers. QtWebEngine (the sign-in page) must be initialized before the
// application object exists.
inline void createApplication(const rust::Vec<rust::String>& arguments)
{
    QtWebEngineQuick::initialize();

    static std::vector<std::string> storage;
    static std::vector<char*> argv;
    static int argc = 0;

    for (const auto& argument : arguments) {
        storage.emplace_back(argument.data(), argument.size());
    }
    for (auto& argument : storage) {
        argv.push_back(argument.data());
    }
    argv.push_back(nullptr);
    argc = static_cast<int>(storage.size());

    detail::application() = std::make_unique<QApplication>(argc, argv.data());
    QApplication::setOrganizationName(QStringLiteral("phonelink"));
    QApplication::setApplicationName(QStringLiteral("phonelink"));
    QApplication::setQuitOnLastWindowClosed(false);
    // The installed .desktop file's name: Wayland compositors match windows to it by this app id, and
    // notifications and the task bar take its icon and name.
    QGuiApplication::setDesktopFileName(QStringLiteral("phonelink-linux"));
    QGuiApplication::setWindowIcon(QIcon::fromTheme(QStringLiteral("phonelink")));
}

inline int execApplication()
{
    return QApplication::exec();
}

// Destroys the application object, after the QML engine and before main() returns: run among the static
// destructors, it would reach Qt's own statics after they are gone (Qt Quick's theme crashes there).
inline void destroyApplication()
{
    detail::application().reset();
}

}
