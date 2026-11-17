//! `wlr-data-control-unstable-v1`, wlroots' data-control protocol, which compositors offered before
//! `ext-data-control-v1`.

use std::os::fd::BorrowedFd;

use wayland_client::QueueHandle;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_protocols_wlr::data_control::v1::client::zwlr_data_control_device_v1::{
    self as device_v1, ZwlrDataControlDeviceV1,
};
use wayland_protocols_wlr::data_control::v1::client::zwlr_data_control_manager_v1::ZwlrDataControlManagerV1;
use wayland_protocols_wlr::data_control::v1::client::zwlr_data_control_offer_v1::{
    self as offer_v1, ZwlrDataControlOfferV1,
};
use wayland_protocols_wlr::data_control::v1::client::zwlr_data_control_source_v1::{
    self as source_v1, ZwlrDataControlSourceV1,
};

use super::events::WaylandState;
use super::{DataControl, DeviceEvent, DeviceUserData, SourceContents, SourceEvent};

/// The protocol `wlr-data-control-unstable-v1`.
pub(in crate::clipboard) enum Wlr {}

impl DataControl for Wlr {
    const MANAGER_INTERFACE: &'static str = "zwlr_data_control_manager_v1";
    const DATA_OFFER_OPCODE: u16 = device_v1::EVT_DATA_OFFER_OPCODE;

    type Manager = ZwlrDataControlManagerV1;
    type Device = ZwlrDataControlDeviceV1;
    type Offer = ZwlrDataControlOfferV1;
    type Source = ZwlrDataControlSourceV1;

    fn get_data_device(
        manager: &ZwlrDataControlManagerV1,
        seat: &WlSeat,
        queue: &QueueHandle<WaylandState<Self>>,
    ) -> ZwlrDataControlDeviceV1 {
        manager.get_data_device(seat, queue, DeviceUserData)
    }

    fn create_data_source(
        manager: &ZwlrDataControlManagerV1,
        queue: &QueueHandle<WaylandState<Self>>,
        contents: SourceContents,
    ) -> ZwlrDataControlSourceV1 {
        manager.create_data_source(queue, contents)
    }

    fn set_selection(device: &ZwlrDataControlDeviceV1, source: &ZwlrDataControlSourceV1) {
        device.set_selection(Some(source));
    }

    fn offer(source: &ZwlrDataControlSourceV1, mime: String) {
        source.offer(mime);
    }

    fn destroy_source(source: &ZwlrDataControlSourceV1) {
        source.destroy();
    }

    fn receive(offer: &ZwlrDataControlOfferV1, mime: String, fd: BorrowedFd<'_>) {
        offer.receive(mime, fd);
    }

    fn destroy_offer(offer: &ZwlrDataControlOfferV1) {
        offer.destroy();
    }

    fn device_event(event: device_v1::Event) -> DeviceEvent<ZwlrDataControlOfferV1> {
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
