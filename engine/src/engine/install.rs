//! Installing the features.

use super::{EngineCore, EngineError, setup_err};
use crate::features::connected_camera::NanoCameraChannels;
use crate::features::installed::{CdehSide, FeatureConnections};
use crate::features::remote_control::DeviceListRefresh;
use crate::features::shared::nano_channels::NanoShareChannels;
use crate::features::shared::peers::DisplayedPeers;
use crate::features::{FeatureEnv, Features, PeerDirectory};
use crate::seams::{PartnerSessionControl, TokenSource};
use std::sync::Arc;
use std::sync::atomic::Ordering;

impl EngineCore {
    /// Installs the features once (`crate::features::installed`), after the platform routes and the
    /// phone selection and before the App layer starts, and bridges the clipboard's global switch to
    /// the advertised capabilities.
    pub(super) fn install_features(self: &Arc<Self>) -> Result<(), EngineError> {
        if self.features.get().is_some() {
            return Ok(());
        }
        if self.install_failed.load(Ordering::SeqCst) {
            return Err(setup_err("an earlier feature install failed part-way; the engine must be rebuilt"));
        }

        // The connection's platform routes must be installed first.
        self.routes().ok_or_else(|| setup_err("the platform routes are not installed"))?;

        let env = FeatureEnv {
            config: self.config.clone(),
            edges: self.edges.clone(),
            app: self.app.clone(),
            sessions: self.sessions.clone(),
            peers: self.auth.clone(),
            switches: self.switches.clone(),
            notifier: self.feature_edges.notifier.clone(),
            responses: self.responses.clone(),
            stores: self.feature_edges.stores.clone(),
            tap: self.tap.clone(),
        };

        // The CDEH features run over the CDEH identity's connection, its device list and its platform
        // routes; the App layer they read (the phone record) stays the primary identity's.
        let (cdeh_auth, cdeh_sessions, cdeh_routes) = self.cdeh_parts();
        let cdeh_device_list = cdeh_auth.clone() as Arc<dyn DeviceListRefresh>;

        // File transfer's and phone files' channels go over the Nano device connection, with the CDEH
        // identity's CloudRelay servers.
        let share_channels = NanoShareChannels::new(self.config.nano.clone(), cdeh_auth.clone());
        // The connected camera's streams share those device connections.
        let camera_channels = NanoCameraChannels::new(share_channels.clone());
        let cdeh_routes =
            cdeh_routes.ok_or_else(|| setup_err("the CDEH identity's platform routes are not installed"))?;

        // Screen mirroring opens its own connection over the App layer's `/nanohandshake`, with the
        // primary identity's CloudRelay servers for the selected phone's environment.
        let mirror_relay: Arc<dyn crate::nano::cloud_relay::RelaySource> = match self.phone() {
            Some(phone) => Arc::new(crate::nano::cloud_relay::CloudRelay::new(
                self.auth.rest().clone(),
                self.auth.clone() as Arc<dyn TokenSource>,
                phone.env,
            )),
            None => Arc::new(crate::nano::cloud_relay::NoRelay),
        };
        let mirror_connector = Arc::new(crate::features::screen_mirroring::NanoMirrorConnector {
            config: self.config.nano.clone(),
            app: self.app.clone(),
            relay: mirror_relay,
        });

        // They serve only the selected phone: the other phones are hidden from them.
        let displayed = DisplayedPeers::new(cdeh_auth as Arc<dyn PeerDirectory>);
        displayed.set_displayed(self.phone());
        let _ = self.displayed_peers.set(displayed.clone());
        let cdeh_env = FeatureEnv {
            sessions: cdeh_sessions as Arc<dyn PartnerSessionControl>,
            peers: displayed as Arc<dyn PeerDirectory>,
            ..env.clone()
        };

        let features = Features::install(
            &env,
            &CdehSide { env: &cdeh_env, routes: &cdeh_routes, device_list: &cdeh_device_list },
            &self.config.features,
            &self.feature_edges,
            FeatureConnections {
                side_channel_auth: self.auth.clone(),
                share_channels: share_channels.clone(),
                files_channels: share_channels,
                camera_channels,
                mirror_connector,
            },
        )
        .map_err(|e| {
            self.install_failed.store(true, Ordering::SeqCst);
            setup_err(format!("feature install: {e}"))
        })?;

        self.spawn_capability_bridge(&features);
        self.events.features_installed(&features);
        crate::features::screen_mirroring::probe::spawn_from_env(features.screen_mirroring.clone());
        let _ = self.features.set(features);
        Ok(())
    }
}
