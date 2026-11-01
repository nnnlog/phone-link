//! Byte-level codecs, pure and synchronous: the `SignalR` `MessagePack` hub protocol, `ms-dcg` packet
//! property maps, the binary inner envelope, the `pbvalueset` `ValueSet`, and ordered JSON objects.

pub mod dcg_packet;
pub mod envelope;
pub mod hub;
pub mod json;
pub mod valueset;

/// The envelope's header names.
pub use crate::types::header;
