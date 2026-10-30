//! Buddy core: configuration, domain models, and data providers.
//!
//! This crate has no UI dependencies so its logic can be unit-tested quickly
//! and shared by any front end.

pub mod application_settings;
pub mod automation;
pub mod bookmarks;
pub mod browser;
pub mod codex_usage;
pub mod config;
pub mod config_lock;
pub mod convex_data;
pub mod copilot_assistant;
pub mod copilot_backend;
mod copilot_fixture;
pub mod copilot_models;
pub mod copilot_results;
pub mod copilot_review;
pub mod copilot_runs;
pub mod copilot_runtime;
pub mod copilot_sdk;
pub mod copilot_sessions;
pub mod copilot_usage;
pub mod crew;
pub mod dashboard;
mod file_lock;
pub mod finance;
pub mod format;
pub mod gh;
pub mod github;
pub mod http;
pub mod pollen;
pub mod pollen_credentials;
pub mod ralph;
pub mod secrets;
pub mod settings_backend;
mod sha256;
pub mod stats;
pub mod terminal;
pub mod terminal_backend;
pub mod terminal_panel;
pub mod terminal_prompts;
pub mod terminal_recovery;
pub mod terminal_workspaces;
pub mod todoist;
pub mod weather;
pub mod zoom;

pub mod tempo;

mod pending_file;

mod account_zone;

pub mod telemetry;
