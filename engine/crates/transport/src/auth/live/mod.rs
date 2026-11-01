//! The Microsoft-account device-bound sign-in that mints the MSA tokens the control plane needs.
//!
//! The user signs in on the account service's own pages; Phone Link on Windows uses the account
//! signed in to Windows instead.
//!
//! It registers this machine as a device with the account service, derives the device key the
//! service hands back, and exchanges the signed-in user's token for a service token per
//! `(scope, client id)`.
//!
//! The cryptographic core is [`license`]; the request shapes are in [`rst`] and [`device`], the
//! sign-in result in [`login`], and the broker that ties them together in [`broker`].

pub mod broker;
pub mod c14n;
pub mod consts;
pub mod device;
pub mod license;
pub mod login;
pub mod rst;
