#pragma once

#include <QtWidgets/QApplication>

#include <memory>
#include <string>
#include <vector>

#include "rust/cxx.h"

namespace phonelink {

// Qt's tray on KDE builds its menu as a QMenu, which needs a QApplication rather than the
// QGuiApplication that cxx-qt-lib offers.
inline void createApplication(const rust::Vec<rust::String>& arguments)
{
    static std::vector<std::string> storage;
    static std::vector<char*> argv;
    static int argc = 0;
    static std::unique_ptr<QApplication> application;

    for (const auto& argument : arguments) {
        storage.emplace_back(argument.data(), argument.size());
    }
    for (auto& argument : storage) {
        argv.push_back(argument.data());
    }
    argv.push_back(nullptr);
    argc = static_cast<int>(storage.size());

    application = std::make_unique<QApplication>(argc, argv.data());
    QApplication::setOrganizationName(QStringLiteral("phonelink"));
    QApplication::setApplicationName(QStringLiteral("phonelink"));
    QApplication::setQuitOnLastWindowClosed(false);
}

inline int execApplication()
{
    return QApplication::exec();
}

}
