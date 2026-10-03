//! Read-only data collection for firstmate fleet homes.
//!
//! Nothing in this crate writes to a home. Local and remote homes are read
//! by one gather script that only runs read commands such as `head`, `tail`,
//! `ls`, `stat` and `ps`; the parsers then turn its output into a
//! [`model::HomeSnapshot`].

pub mod backlog;
pub mod bundle;
pub mod collect;
pub mod config;
pub mod ledger;
pub mod meta;
pub mod model;
pub mod pr;
pub mod projects;
pub mod routes;
pub mod status;
pub mod transport;
