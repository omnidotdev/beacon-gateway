//! Service discovery using mDNS/DNS-SD
//!
//! Advertises the beacon gateway on the local network so clients can
//! discover it without manual configuration

pub mod mdns;

pub use mdns::MdnsAdvertiser;
