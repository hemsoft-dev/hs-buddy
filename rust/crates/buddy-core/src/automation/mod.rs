//! Scheduled work uses the server's queue and captured, claim-bound executions.
pub mod ai;
pub mod backend;
pub mod cron;
pub mod edit;
pub mod engine;
pub mod exec;
pub mod forecast;
mod host;
pub mod journal;
pub mod models;
pub mod snapshot;
pub mod trigger;
pub mod workers;

pub use host::run_automation_host;
