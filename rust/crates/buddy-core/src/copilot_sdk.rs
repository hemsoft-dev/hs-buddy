//! SDK process configuration. The caller supplies a verified packaged runtime
//! and a dedicated owned state directory, separate from the session reader.
use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::time::Duration;

use github_copilot_sdk::handler::DenyAllHandler;
use github_copilot_sdk::session::Session;
use github_copilot_sdk::types::{SessionConfig, SessionEvent};
use github_copilot_sdk::{CliProgram, Client, ClientMode, ClientOptions, LogLevel, Transport};

use crate::copilot_runs::{Execution, Failure, Phase, RunKey};

/// Owns one isolated SDK child. Session handles may retain SDK client clones;
/// dropping this owner still terminates its child instead of awaiting those
/// clones. Call `stop` to await orderly shutdown before dropping it.
pub struct AssistantRuntime {
    client: Client,
    fixture: Option<std::sync::Arc<crate::copilot_fixture::Fixture>>,
}

impl AssistantRuntime {
    /// Starts transport only. Model turns require an authenticated session and
    /// an explicit send claimed by the application's execution state.
    pub async fn start(
        verified_runtime: &Path,
        state_root: &Path,
        credential: Option<String>,
    ) -> Result<Self, Failure> {
        Self::start_with_fixture(verified_runtime, state_root, credential, None).await
    }
    pub(crate) async fn start_with_fixture(
        verified_runtime: &Path,
        state_root: &Path,
        credential: Option<String>,
        fixture: Option<std::sync::Arc<crate::copilot_fixture::Fixture>>,
    ) -> Result<Self, Failure> {
        let mut options = client_options(
            verified_runtime,
            state_root,
            credential,
            std::env::vars_os().map(|(name, _)| name),
        )
        .map_err(|_| Failure::ServiceUnavailable)?;
        if let Some(fixture) = &fixture {
            options.request_handler = Some(fixture.clone());
            options.on_list_models = Some(fixture.clone());
        }
        let client = tokio::time::timeout(Duration::from_secs(45), Client::start(options))
            .await
            .map_err(|_| Failure::ServiceUnavailable)?
            .map_err(|_| Failure::ServiceUnavailable)?;
        Ok(Self { client, fixture })
    }

    /// Never silently use a different logged-in account. No model request is
    /// sent by session creation. The session has no advertised or approved tools.
    pub async fn create_session(&self, account: &str, model: &str) -> Result<Session, Failure> {
        if !crate::copilot_runs::account_slug(account) {
            return Err(Failure::InvalidContext);
        }
        let mut config = session_config(model)?;
        if let Some(fixture) = &self.fixture {
            fixture.identity(account, Some(model))?;
            config.provider = Some(fixture.provider());
        }
        self.require_account(account).await?;
        match tokio::time::timeout(Duration::from_secs(30), self.client.create_session(config))
            .await
        {
            Ok(Ok(session)) => Ok(session),
            _ => {
                self.client.force_stop();
                Err(Failure::ServiceUnavailable)
            }
        }
    }

    async fn require_account(&self, account: &str) -> Result<(), Failure> {
        if let Some(fixture) = &self.fixture {
            return fixture.identity(account, None);
        }
        if !crate::copilot_runs::account_slug(account) {
            return Err(Failure::InvalidContext);
        }
        let auth = tokio::time::timeout(Duration::from_secs(10), self.client.get_auth_status())
            .await
            .map_err(|_| Failure::ServiceUnavailable)?
            .map_err(|_| Failure::ServiceUnavailable)?;
        if !auth.is_authenticated
            || !auth
                .login
                .as_ref()
                .is_some_and(|login| login.eq_ignore_ascii_case(account))
        {
            return Err(Failure::AuthenticationRequired);
        }
        Ok(())
    }

    pub async fn models(
        &self,
        account: &str,
    ) -> Result<Vec<crate::copilot_models::Model>, Failure> {
        self.require_account(account).await?;
        let models = tokio::time::timeout(Duration::from_secs(30), self.client.list_models())
            .await
            .map_err(|_| Failure::ServiceUnavailable)?
            .map_err(|_| Failure::ServiceUnavailable)?;
        crate::copilot_models::from_sdk(models)
    }

    pub async fn stop(&self) -> Result<(), Failure> {
        match tokio::time::timeout(Duration::from_secs(15), self.client.stop()).await {
            Ok(Ok(())) => Ok(()),
            _ => {
                self.client.force_stop();
                Err(Failure::ServiceUnavailable)
            }
        }
    }

    /// Cancellation and app shutdown can stop the owned process synchronously.
    /// SDK handles retained by background tasks cannot keep it running.
    pub fn force_stop(&self) {
        self.client.force_stop();
    }
}

impl Drop for AssistantRuntime {
    fn drop(&mut self) {
        self.client.force_stop();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchError {
    RuntimeUnavailable,
    InvalidStateDirectory,
    StateUnavailable,
    InvalidCredential,
}

impl std::fmt::Display for LaunchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::RuntimeUnavailable => "The packaged assistant runtime is unavailable.",
            Self::InvalidStateDirectory => "The assistant state directory must be absolute.",
            Self::StateUnavailable => {
                "The private assistant state directory could not be prepared."
            }
            Self::InvalidCredential => "The selected account credential is invalid.",
        })
    }
}

impl std::error::Error for LaunchError {}

pub(crate) fn private_directory(path: &Path) -> Result<(), LaunchError> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(path)
        .map_err(|_| LaunchError::StateUnavailable)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| LaunchError::StateUnavailable)?;
    }
    Ok(())
}

fn same_environment_name(left: &OsStr, right: &OsStr) -> bool {
    #[cfg(windows)]
    {
        left.to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}

/// No ambient login or executable resolution. An absent credential injects an
/// empty token for health/offline qualification; it never inherits another one.
/// This function does not create a session or authorize sending a model turn.
pub fn client_options(
    verified_runtime: &Path,
    state_root: &Path,
    credential: Option<String>,
    ambient_names: impl IntoIterator<Item = OsString>,
) -> Result<ClientOptions, LaunchError> {
    if !verified_runtime.is_absolute() || !verified_runtime.is_file() {
        return Err(LaunchError::RuntimeUnavailable);
    }
    if !state_root.is_absolute() {
        return Err(LaunchError::InvalidStateDirectory);
    }
    if credential.as_ref().is_some_and(|token| {
        token.len() > 16 * 1024 || !token.bytes().all(|byte| byte.is_ascii_graphic())
    }) {
        return Err(LaunchError::InvalidCredential);
    }
    private_directory(state_root)?;
    let mut options = ClientOptions::default();
    options.program = CliProgram::Path(verified_runtime.to_owned());
    options.mode = ClientMode::Empty;
    options.transport = Transport::Stdio;
    options.log_level = Some(LogLevel::None);
    options.use_logged_in_user = Some(false);
    options.github_token = Some(credential.unwrap_or_default());
    options.base_directory = Some(state_root.join("state"));
    options.working_directory = state_root.join("working");
    private_directory(options.base_directory.as_ref().unwrap())?;
    private_directory(&options.working_directory)?;
    for (key, folder) in [
        ("HOME", "home"),
        ("USERPROFILE", "home"),
        ("APPDATA", "appdata"),
        ("LOCALAPPDATA", "local-appdata"),
        ("XDG_CONFIG_HOME", "xdg-config"),
        ("XDG_CACHE_HOME", "xdg-cache"),
        ("GH_CONFIG_DIR", "gh"),
    ] {
        let directory = state_root.join(folder);
        private_directory(&directory)?;
        options.env.push((key.into(), directory.into_os_string()));
    }
    for (key, value) in [
        ("COPILOT_DISABLE_KEYTAR", "1"),
        ("COPILOT_GITHUB_TELEMETRY_DISABLED", "true"),
        ("COPILOT_OTEL_ENABLED", "false"),
        (
            "OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT",
            "false",
        ),
    ] {
        options.env.push((key.into(), value.into()));
    }
    let mut removals = BTreeSet::new();
    for name in ambient_names.into_iter().chain(
        [
            "GH_TOKEN",
            "GITHUB_TOKEN",
            "GH_ENTERPRISE_TOKEN",
            "GITHUB_ENTERPRISE_TOKEN",
            "COPILOT_GITHUB_TOKEN",
            "COPILOT_CLI_PATH",
            "COPILOT_RUNTIME_PATH",
            "COPILOT_TRANSPORT",
            "OPENAI_API_KEY",
            "ANTHROPIC_API_KEY",
            "AZURE_OPENAI_API_KEY",
            "NODE_OPTIONS",
            "NODE_PATH",
            "DEBUG",
            "RUST_LOG",
        ]
        .into_iter()
        .map(OsString::from),
    ) {
        // The SDK injects these before applying env_remove. Removing them here
        // would strip the selected token or discard the owned SDK home.
        let reserved = [
            OsStr::new("COPILOT_SDK_AUTH_TOKEN"),
            OsStr::new("COPILOT_HOME"),
        ]
        .iter()
        .any(|key| same_environment_name(&name, key))
            || options
                .env
                .iter()
                .any(|(key, _)| same_environment_name(&name, key));
        let upper = name.to_string_lossy().to_ascii_uppercase();
        let sensitive = [
            "GH_",
            "GITHUB_",
            "COPILOT_",
            "OPENAI_",
            "ANTHROPIC_",
            "AZURE_",
            "CODEX_",
            "OTEL_",
        ]
        .iter()
        .any(|prefix| upper.starts_with(prefix))
            || matches!(
                upper.as_str(),
                "NODE_OPTIONS" | "NODE_PATH" | "DEBUG" | "RUST_LOG"
            );
        if sensitive && !reserved {
            removals.insert(name);
        }
    }
    options.env_remove = removals.into_iter().collect();
    Ok(options)
}

/// Chat sessions receive no filesystem, terminal, network or custom tools.
/// Both the advertised tool list and permission handler enforce this boundary.
pub fn session_config(model: &str) -> Result<SessionConfig, Failure> {
    if model.is_empty() || model.len() > 256 || model.chars().any(char::is_control) {
        return Err(Failure::InvalidContext);
    }
    let mut config =
        SessionConfig::default().with_permission_handler(std::sync::Arc::new(DenyAllHandler));
    config.model = Some(model.to_owned());
    config.allowed_models = Some(vec![model.to_owned()]);
    config.streaming = Some(true);
    config.available_tools = Some(vec![]);
    Ok(config)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventAction {
    Ignored,
    Changed,
    Finished,
    AbortRequired,
}

/// Translate SDK events without exposing raw event/error bodies to logs or UI.
/// A dedicated session subscription belongs to one captured execution key.
/// Receive errors and lag must separately fail the execution and stop the turn.
pub fn apply_event(execution: &mut Execution, key: RunKey, event: &SessionEvent) -> EventAction {
    if execution.key() != key || event.agent_id.is_some() {
        return EventAction::Ignored;
    }
    let changed = match event.event_type.as_str() {
        "assistant.message_delta" | "assistant.message" => {
            if !matches!(execution.phase(), Phase::Sending | Phase::Streaming) {
                return EventAction::Ignored;
            }
            let field = if event.event_type == "assistant.message_delta" {
                "deltaContent"
            } else {
                "content"
            };
            let (Some(id), Some(text)) =
                (event.data["messageId"].as_str(), event.data[field].as_str())
            else {
                execution.failed(key, Failure::IncompleteStream);
                return EventAction::AbortRequired;
            };
            if event.event_type == "assistant.message_delta" {
                execution.delta(key, id, text)
            } else {
                execution.final_message(key, id, text)
            }
        }
        "session.idle" => execution.idle(key),
        "session.error" => execution.failed(key, Failure::ServiceUnavailable),
        _ => return EventAction::Ignored,
    };
    match execution.phase() {
        Phase::Failed if changed || execution.failure().is_some() => EventAction::AbortRequired,
        Phase::Completed | Phase::AwaitingPersistence | Phase::Cancelled if changed => {
            EventAction::Finished
        }
        _ if changed => EventAction::Changed,
        _ => EventAction::Ignored,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::copilot_runs::Context;
    use serde_json::json;

    fn execution() -> Execution {
        let mut execution = Execution::new(
            Context {
                account: "synthetic-account".into(),
                model: "gpt-4.1".into(),
                review: None,
            },
            "synthetic prompt".into(),
            false,
        )
        .unwrap();
        execution.begin_send().unwrap();
        execution
    }

    fn event(kind: &str, data: serde_json::Value) -> SessionEvent {
        serde_json::from_value(json!({"id":"synthetic-event", "timestamp":"2026-10-16T13:00:00Z", "type":kind, "data":data})).unwrap()
    }

    #[test]
    fn malformed_delta_retains_partial_and_requires_abort() {
        let mut run = execution();
        let key = run.key();
        assert!(run.delta(key, "message-1", "private partial"));
        assert_eq!(
            apply_event(
                &mut run,
                key,
                &event(
                    "assistant.message_delta",
                    json!({"deltaContent":"missing identity"})
                )
            ),
            EventAction::AbortRequired
        );
        assert_eq!(run.failure(), Some(Failure::IncompleteStream));
        assert_eq!(run.output(), "private partial");
        assert!(run.begin_send().is_err());
    }

    #[test]
    fn stale_keys_and_subagent_output_cannot_change_the_answer() {
        let mut run = execution();
        let key = run.key();
        let other = execution().key();
        let mut message = event(
            "assistant.message",
            json!({"messageId":"message-1", "content":"foreign output"}),
        );
        assert_eq!(apply_event(&mut run, other, &message), EventAction::Ignored);
        message.agent_id = Some("nested-agent".into());
        assert_eq!(apply_event(&mut run, key, &message), EventAction::Ignored);
        assert_eq!(run.output(), "");
        assert_eq!(run.phase(), Phase::Sending);
    }

    #[test]
    fn idle_cannot_complete_a_truncated_response() {
        let mut run = execution();
        let key = run.key();
        assert!(run.delta(key, "message-1", "partial"));
        assert_eq!(
            apply_event(&mut run, key, &event("session.idle", json!({}))),
            EventAction::AbortRequired
        );
        assert_eq!(run.phase(), Phase::Failed);
        assert_eq!(run.output(), "partial");
    }

    #[test]
    fn raw_service_error_becomes_static_failure_without_replay() {
        let mut run = execution();
        let key = run.key();
        assert_eq!(
            apply_event(
                &mut run,
                key,
                &event(
                    "session.error",
                    json!({"message":"private-provider-secret"})
                )
            ),
            EventAction::AbortRequired
        );
        assert_eq!(run.failure(), Some(Failure::ServiceUnavailable));
        assert!(
            !run.failure()
                .unwrap()
                .to_string()
                .contains("private-provider-secret")
        );
        assert!(run.begin_send().is_err());
    }

    #[test]
    fn rejects_invalid_models_before_session_creation() {
        for invalid in ["", "gpt\nsecret", &"x".repeat(257)] {
            assert!(matches!(
                session_config(invalid),
                Err(Failure::InvalidContext)
            ));
        }
        let config = session_config("gpt-4.1").unwrap();
        assert_eq!(config.available_tools, Some(vec![]));
        assert_eq!(config.allowed_models, Some(vec!["gpt-4.1".to_owned()]));
        assert_eq!(config.streaming, Some(true));
    }
}
