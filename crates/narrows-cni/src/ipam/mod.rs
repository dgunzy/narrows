//! IP address management: which pod gets which address (PLAN §5.3).
//!
//! In Phase 1 this lives inside the fat plugin. Phase 2 moves it into the
//! agent, which becomes the single owner of lease state.
//!
//! This slice covers the pure logic: CIDR arithmetic, the allocator, the
//! `ipam` config section, and a locked, atomically-written lease file.
//! Quarantine and the conntrack flush on release come in later slices.

pub mod allocator;
pub mod cidr;
pub mod config;
pub mod store;

pub use allocator::{Allocator, IpamError, LeaseKey};
pub use cidr::{CidrError, Ipv4Cidr};
pub use config::IpamConfig;
pub use store::{Store, StoreError};
