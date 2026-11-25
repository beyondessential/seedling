//! The Linux platform of the Seedling runtime: the adapters through which the
//! portable runtime in `seedling-core` reaches a Linux host.

mod confined;
pub mod connector;
pub mod data_plane;
pub mod host;
pub mod jool;
pub mod journal;
pub mod netinfo;
pub mod podman;
pub mod systemd;
pub mod volume_storage;
