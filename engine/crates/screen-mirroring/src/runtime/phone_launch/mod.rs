//! Launches the phone asks for: the remoting URI of a `phoneCommand` `LaunchUri` (the phone's
//! "Mirror to PC"), and where each one leads.

use tokio::sync::broadcast;
use tracing::{info, warn};

use super::Inner;
use crate::ScreenMirroringEvent;
use crate::gates::AppsFeatureType;
use phonelink_app::AppEvent;
use phonelink_app::shared::RemotingUri;
use phonelink_app::shared::remoting_uri::PHONE_SCREEN_PACKAGE;

/// The `launchsource` values that, with app windows, show the Apps view instead of launching.
const APPS_VIEW_SOURCES: [&str; 2] = ["Taskbar", "StartMenu"];

/// Where a remoting URI leads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RemotingTarget {
    /// What any other `ms-phone` URI does: the main window comes forward.
    MainWindow,
    /// The Apps view.
    AppsView,
    /// The phone-screen window.
    PhoneScreen,
    /// A window of this phone app (the phone screen without app windows).
    App(String),
}

/// The target of `uri` for a phone with these gates.
pub(crate) fn remoting_target(
    uri: &RemotingUri,
    remoting_supported: bool,
    phone_screen_apps: bool,
    apps: AppsFeatureType,
) -> RemotingTarget {
    if !remoting_supported {
        return RemotingTarget::MainWindow;
    }

    let package = uri.package_name.as_deref().filter(|p| !p.trim().is_empty());
    let Some(package) = package.filter(|_| phone_screen_apps) else {
        return RemotingTarget::AppsView;
    };

    let source = uri.launch_source.as_deref().unwrap_or_default();
    if apps == AppsFeatureType::MultipleApps && APPS_VIEW_SOURCES.contains(&source) {
        RemotingTarget::AppsView
    } else if package == PHONE_SCREEN_PACKAGE {
        RemotingTarget::PhoneScreen
    } else {
        RemotingTarget::App(package.to_owned())
    }
}

impl Inner {
    /// Serves every remoting launch the App layer reports for the selected phone.
    pub(super) fn spawn_phone_launches(&self, mut events: broadcast::Receiver<AppEvent>) {
        let weak = self.weak.clone();
        self.spawn(async move {
            loop {
                match events.recv().await {
                    Ok(AppEvent::RemotingLaunch { phone, uri }) => {
                        let Some(me) = weak.upgrade() else { break };
                        if me.phone().is_ok_and(|p| p.is_same_client(&phone)) {
                            me.launch_from_phone(&uri);
                        }
                    }
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(n, "app events lagged; a launch the phone asked for may be lost")
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }

    /// Launches what `uri` names, as a PC launch.
    fn launch_from_phone(&self, uri: &RemotingUri) {
        let target = match self.values() {
            Ok(v) => remoting_target(
                uri,
                v.remoting_supported(),
                v.phone_screen_apps(),
                v.apps_feature_type(self.separate_window()),
            ),
            Err(_) => RemotingTarget::MainWindow,
        };

        info!(target_kind = target.kind(), "launch asked by the phone");
        let launched = match target {
            RemotingTarget::MainWindow => {
                let _ = self.events.send(ScreenMirroringEvent::ShowMainWindow);
                return;
            }
            RemotingTarget::AppsView => {
                let _ = self.events.send(ScreenMirroringEvent::ShowApps);
                return;
            }
            RemotingTarget::PhoneScreen => self.open_phone_screen(None),
            RemotingTarget::App(package) => self.launch_app(&package, None),
        };

        if let Err(e) = launched {
            warn!(error = %e, "launch asked by the phone not started");
        }
    }
}

impl RemotingTarget {
    /// The target's name for the log, without the package.
    fn kind(&self) -> &'static str {
        match self {
            Self::MainWindow => "main_window",
            Self::AppsView => "apps_view",
            Self::PhoneScreen => "phone_screen",
            Self::App(_) => "app",
        }
    }
}

#[cfg(test)]
mod tests;
