//! Buddy core: configuration, domain models, and data providers.
//!
//! This crate has no UI dependencies so its logic can be unit-tested quickly
//! and shared by any front end.

pub mod application_settings;
pub mod config;
pub mod config_lock;
pub mod convex_data;
pub mod copilot_models;
pub mod copilot_usage;
pub mod dashboard;
pub mod finance;
pub mod format;
pub mod gh;
pub mod github;
pub mod http;
pub mod pollen;
pub mod pollen_credentials;
pub mod secrets;
pub mod settings_backend;
pub mod stats;
pub mod weather;
pub mod zoom;
