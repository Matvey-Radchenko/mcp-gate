pub mod advisor;
pub mod backend;
pub mod catalog;
pub mod clients;
mod concurrency;
pub mod config;
pub mod deployment;
mod discovery;
mod dispatch;
mod execution;
pub mod gateway;
pub mod http;
pub mod install;
pub mod manage;
mod ownership;
pub mod platform;
pub mod policy;
mod process;
mod recovery;
pub mod worker;

/// Abbreviated Git commit for builds from a checkout, when available.
pub const BUILD_COMMIT: Option<&str> = option_env!("MCP_GATE_BUILD_COMMIT");
