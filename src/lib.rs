//! The Grindvakt app: CLI, interactive vault flows, unattended credentials and OS services.
//! The DNS engine, vault format and private-file handling come from
//! [grindvakt-core](https://crates.io/crates/grindvakt-core), pinned to this app's version.
pub mod cli;
pub mod config;
pub mod fsutil;
pub mod ip;
pub mod lock;
pub mod provider;
pub mod schedule;
pub mod service;
pub mod sync;
pub mod vault;
#[cfg(target_os = "windows")]
pub mod winservice;
#[cfg(target_os = "windows")]
pub mod winutil;
