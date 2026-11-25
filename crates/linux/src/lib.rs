//! The Linux platform of the Seedling runtime: the adapters through which the
//! portable runtime in `seedling-core` reaches a Linux host.

mod confined;
pub mod connector;
pub mod journal;
pub mod podman;
pub mod systemd;
pub mod volume_storage;
