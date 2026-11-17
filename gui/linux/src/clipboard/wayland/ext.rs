//! `ext-data-control-v1`, the data-control protocol of `wayland-protocols`.

use std::os::fd::BorrowedFd;

use wayland_client::QueueHandle;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_protocols::ext::data_control::v1::client::ext_data_control_device_v1::{
    self as device_v1, ExtDataControlDeviceV1,
};
use wayland_protocols::ext::data_control::v1::client::ext_data_control_manager_v1::ExtDataControlManagerV1;
use wayland_protocols::ext::data_control::v1::client::ext_data_control_offer_v1::{
    self as offer_v1, ExtDataControlOfferV1,
};
use wayland_protocols::ext::data_control::v1::client::ext_data_control_source_v1::{
    self as source_v1, ExtDataControlSourceV1,
};

use super::events::WaylandState;
use super::{DataControl, DeviceEvent, DeviceUserData, SourceContents, SourceEvent};

/// The protocol `ext-data-control-v1`.
pub(in crate::clipboard) enum Ext {}

impl DataControl for Ext {
    const MANAGER_INTERFACE: &'static str = "ext_data_control_manager_v1";
    const DATA_OFFER_OPCODE: u16 = device_v1::EVT_DATA_OFFER_OPCODE;

    type Manager = ExtDataControlManagerV1;
    type Device = ExtDataControlDeviceV1;
    type Offer = ExtDataControlOfferV1;
    type Source = ExtDataControlSourceV1;

    fn get_data_device(
        manager: &ExtDataControlManagerV1,
        seat: &WlSeat,
        queue: &QueueHandle<WaylandState<Self>>,
    ) -> ExtDataControlDeviceV1 {
        manager.get_data_device(seat, queue, DeviceUserData)
    }

    fn create_data_source(
        manager: &ExtDataControlManagerV1,
        queue: &QueueHandle<WaylandState<Self>>,
        contents: SourceContents,
    ) -> ExtDataControlSourceV1 {
        manager.create_data_source(queue, contents)
    }

    fn set_selection(device: &ExtDataControlDeviceV1, source: &ExtDataControlSourceV1) {
        device.set_selection(Some(source));
    }

    fn offer(source: &ExtDataControlSourceV1, mime: String) {
        source.offer(mime);
    }

    fn destroy_source(source: &ExtDataControlSourceV1) {
        source.destroy();
    }

    fn receive(offer: &ExtDataControlOfferV1, mime: String, fd: BorrowedFd<'_>) {
        offer.receive(mime, fd);
    }

    fn destroy_offer(offer: &ExtDataControlOfferV1) {
        offer.destroy();
    }

    fn device_event(event: device_v1::Event) -> DeviceEvent<ExtDataControlOfferV1> {
        match event {
            device_v1::Event::DataOffer { id } => DeviceEvent::DataOffer(id),
            device_v1::Event::Selection { id } => DeviceEvent::Selection(id),
            device_v1::Event::PrimarySelection { id } => DeviceEvent::PrimarySelection(id),
            _ => DeviceEvent::Other,
        }
    }

    fn offered_mime(event: offer_v1::Event) -> Option<String> {
        match event {
            offer_v1::Event::Offer { mime_type } => Some(mime_type),
            _ => None,
        }
    }

    fn source_event(event: source_v1::Event) -> SourceEvent {
        match event {
            source_v1::Event::Send { mime_type, fd } => SourceEvent::Send { mime_type, fd },
            source_v1::Event::Cancelled => SourceEvent::Cancelled,
            _ => SourceEvent::Other,
        }
    }
}
