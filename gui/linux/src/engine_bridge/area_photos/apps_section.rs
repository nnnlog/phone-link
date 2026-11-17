//! The Apps section: what it shows (its lists, each tile with its app's notification count, the search,
//! the icons), its actions (open an app or a recent task's document, the favorites, allowing the
//! phone's apps here), and the phone apps feature's changes.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

use cxx_qt::{CxxQtType, Threading};
use phonelink_engine::api::engine::Engine;
use phonelink_engine::api::phone_apps::PhoneAppsChange;
use serde_json::{Value, json};
use tracing::{debug, error, warn};

use super::files::{self, OpenError};
use super::work::{WorkStopped, blocking};
use super::{apps, quiet};
use crate::engine_bridge::EngineBridge;
use crate::engine_bridge::features::{Failure, Revision, fail_later};
use crate::images;
use crate::l10n::translate;

/// Why a recent app's document did not open on this computer.
#[derive(Debug, thiserror::Error)]
enum DocumentOpenError {
    #[error(transparent)]
    NotOpened(#[from] OpenError),
    #[error(transparent)]
    Stopped(#[from] WorkStopped),
}

/// The `apps` list (every app the phone lists, in the order of the user's language): the screen
/// mirroring windows read the names.
pub(super) fn apps_list(engine: &Arc<Engine>) -> Value {
    let list = engine.clone().phone_apps().apps().unwrap_or_default();
    Value::Array(
        list.iter()
            .map(
                |a| json!({ "package": a.package_name, "name": a.name, "hasIcon": a.has_icon, "favorite": a.favorite }),
            )
            .collect(),
    )
}

pub(super) fn app_icon(engine: &Arc<Engine>, generation: u64, package: &str) -> Option<String> {
    images::cached("app-icons", &format!("{generation}-{package}"), || {
        engine.clone().phone_apps().icon(package.to_owned()).ok().flatten()
    })
}

/// Each app's notification count, by lower-cased package; an app whose notifications are off (not
/// listed nor shown as banners) counts none, and an app whose setting can't be read counts.
fn notification_counts(engine: &Arc<Engine>) -> HashMap<String, usize> {
    let notifications = engine.clone().notifications();
    let list = notifications.list().unwrap_or_default();
    apps::notification_counts(&list, |package| match notifications.app_setting(package.to_owned()) {
        Ok(setting) => setting.flags.feed || setting.flags.banner,
        Err(e) => {
            debug!(error = %e, "an app's notification setting was not read; its tile shows the count");
            true
        }
    })
}

impl EngineBridge {
    /// The Apps section: whether it shows, and its lists once the phone's lists are not behind.
    pub(super) fn apps_view(&self, engine: &Arc<Engine>) -> Value {
        let api = engine.clone().phone_apps();
        let mut memory = self.areas.photos.apps.borrow_mut();
        let state = match api.state() {
            Ok(state) => state,
            Err(e) => {
                if !quiet(&e) {
                    warn!(error = %e, "the phone apps state was not read");
                }
                *memory = apps::Memory::default();
                return Value::Null;
            }
        };

        let phone = engine.clone().phone().info().ok().map(|info| info.id);
        let mut first_shown = false;
        if !state.syncing && memory.lists_shown_for != phone {
            memory.lists_shown_for.clone_from(&phone);
            first_shown = true;
        }
        let lists_shown = phone.is_some() && memory.lists_shown_for == phone;

        // The phone's screen and apps may open on this computer (the Phone Screen setting).
        let allow_apps = engine.clone().screen_mirroring().status().map_or(true, |status| status.settings.allow_apps);
        let mut view = json!({
            "visible": state.apps_view_visible,
            "allowApps": allow_apps,
            "listsShown": lists_shown,
            "syncing": state.syncing,
            "showsRecent": state.recent_apps_supported,
        });
        if !lists_shown {
            return view;
        }

        let (all, favorites, recent) = match (api.apps(), api.favorites(), api.recent()) {
            (Ok(all), Ok(favorites), Ok(recent)) => (all, favorites, recent),
            (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => {
                warn!(error = %e, "the phone's apps were not read");
                return view;
            }
        };
        let counts = notification_counts(engine);
        let recent_counts = memory.recent_counts(&recent, &counts, first_shown);

        view["searchOffered"] = json!(all.len() >= apps::SEARCH_MINIMUM_APPS);
        view["apps"] = all.iter().map(|app| apps::app_tile(app, &counts)).collect();
        view["favorites"] = favorites.iter().map(|app| apps::app_tile(app, &counts)).collect();
        view["recent"] = recent
            .iter()
            .map(|entry| apps::recent_tile(entry, recent_counts.get(&entry.recent_id).copied().unwrap_or(0)))
            .collect();
        view
    }

    pub(super) fn apps_search(&self, engine: &Arc<Engine>, query: &str) -> Value {
        match engine.clone().phone_apps().search(query.to_owned()) {
            Ok(found) => {
                let counts = notification_counts(engine);
                found.iter().map(|app| apps::app_tile(app, &counts)).collect()
            }
            Err(e) => {
                if !quiet(&e) {
                    warn!(error = %e, "the phone's apps were not searched");
                }
                json!([])
            }
        }
    }

    pub(super) fn perform_apps(mut self: Pin<&mut Self>, engine: Arc<Engine>, name: &str, arg: &str) {
        let api = engine.clone().phone_apps();
        let request: Value = serde_json::from_str(arg).unwrap_or(Value::Null);
        let done: Result<(), Failure> = match name {
            "apps.launch" => {
                let package = request["package"].as_str().unwrap_or(arg).to_owned();
                let task = request["taskId"].as_i64();
                engine
                    .screen_mirroring()
                    .launch_app(package, task)
                    .map(drop)
                    .map_err(|e| Failure::feature("Couldn't Open the App", &e))
            }
            "apps.openRecent" | "apps.openDocument" => self.as_mut().open_recent(&engine, name, &request),
            "apps.addFavorite" => {
                api.add_favorite(arg.to_owned()).map_err(|e| Failure::feature("Couldn't Add to Favorites", &e))
            }
            "apps.removeFavorite" | "apps.dropOnAllApps" => {
                let favorite = api.favorites().unwrap_or_default().iter().any(|app| app.package_name == arg);
                if favorite || name == "apps.removeFavorite" {
                    api.remove_favorite(arg.to_owned())
                        .map_err(|e| Failure::feature("Couldn't Remove from Favorites", &e))
                } else {
                    Ok(())
                }
            }
            "apps.moveFavorite" | "apps.dropOnFavorites" => {
                let package = request["package"].as_str().unwrap_or_default();
                let index = request["index"].as_u64().and_then(|index| usize::try_from(index).ok());
                let favorites: Vec<String> =
                    api.favorites().unwrap_or_default().into_iter().map(|a| a.package_name).collect();
                let all: Vec<String> = api.apps().unwrap_or_default().into_iter().map(|a| a.package_name).collect();
                let change = if name == "apps.moveFavorite" {
                    index.and_then(|index| apps::moved(&favorites, package, index)).map(apps::FavoritesDrop::Move)
                } else {
                    apps::drop_on_favorites(&favorites, &all, package, index)
                };
                match change {
                    Some(apps::FavoritesDrop::Move(order)) => {
                        api.reorder_favorites(order).map_err(|e| Failure::feature("Couldn't Reorder Favorites", &e))
                    }
                    Some(apps::FavoritesDrop::Add) => api
                        .add_favorite(package.to_owned())
                        .map_err(|e| Failure::feature("Couldn't Add to Favorites", &e)),
                    None => Ok(()),
                }
            }
            "apps.allowApps" => {
                let mirroring = engine.screen_mirroring();
                mirroring
                    .status()
                    .and_then(|status| {
                        let mut settings = status.settings;
                        settings.allow_apps = true;
                        mirroring.set_settings(settings)
                    })
                    .map_err(|e| Failure::setting(&e))
            }
            other => {
                warn!(action = other, "unknown action");
                Ok(())
            }
        };

        if let Err(failure) = done {
            self.as_mut().show_failure(failure);
        }
        match name {
            "apps.launch" | "apps.openRecent" | "apps.openDocument" => {}
            "apps.allowApps" => self.bump(Revision::Mirroring),
            _ => self.bump(Revision::Apps),
        }
    }

    /// A click on a recent app: when the phone hands documents over, a task's document opens on this
    /// computer, else in the browser; otherwise the app (or its task) opens from the phone.
    /// `apps.openDocument` opens the document `where` names (`desktop` or `browser`).
    fn open_recent(self: Pin<&mut Self>, engine: &Arc<Engine>, name: &str, request: &Value) -> Result<(), Failure> {
        let api = engine.clone().phone_apps();
        let Some(id) = request["recentId"].as_i64() else {
            warn!(action = name, arg = %request, "the action takes a recent id");
            return Ok(());
        };
        let recent = api.recent().map_err(|e| Failure::feature("Couldn't Open the App", &e))?;
        let Some(entry) = recent.into_iter().find(|entry| entry.recent_id == id) else {
            debug!(action = name, "the action on a recent app the phone no longer lists is ignored");
            return Ok(());
        };

        let present = |uri: &Option<String>| uri.clone().filter(|uri| !uri.is_empty());
        let document = if name == "apps.openDocument" {
            match request["where"].as_str() {
                Some("desktop") => present(&entry.desktop_uri),
                Some("browser") => present(&entry.web_uri),
                other => {
                    warn!(place = ?other, "apps.openDocument opens on the desktop or in the browser, not at the place named");
                    return Ok(());
                }
            }
        } else if api.state().is_ok_and(|state| state.app_handoff_enabled) {
            present(&entry.desktop_uri).or_else(|| present(&entry.web_uri))
        } else {
            None
        };

        match document {
            Some(uri) => {
                let thread = self.qt_thread();
                self.runtime.spawn(async move {
                    let opened = blocking(move || files::open_externally(&uri).map_err(DocumentOpenError::from)).await;
                    if let Err(e) = opened {
                        error!(error = %e, "a recent app's document was not opened");
                        let message = translate("No app on this computer opened it.").to_owned();
                        fail_later(&thread, Failure::new("Couldn't Open the Document", message));
                    }
                });
                Ok(())
            }
            None if name == "apps.openDocument" => Ok(()),
            None => {
                let task = (!entry.main_activity).then_some(entry.task_id);
                engine
                    .clone()
                    .screen_mirroring()
                    .launch_app(entry.package_name, task)
                    .map(drop)
                    .map_err(|e| Failure::feature("Couldn't Open the App", &e))
            }
        }
    }

    pub(in crate::engine_bridge) fn apps_changed(mut self: Pin<&mut Self>, change: PhoneAppsChange) {
        match change {
            PhoneAppsChange::AppsChanged | PhoneAppsChange::StateChanged => {
                images::clear_kind("app-icons");
                self.as_mut().rust_mut().areas.photos.icon_generation += 1;
            }
            PhoneAppsChange::RecentChanged => self.as_mut().rust_mut().areas.photos.apps.get_mut().recent_stale = true,
        }
        self.bump(Revision::Apps);
    }
}
