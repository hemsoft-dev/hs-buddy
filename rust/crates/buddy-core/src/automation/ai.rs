//! Scheduled agent turns have their own captured identity and tool policy.
//! This does not change the chat runtime's deny-all policy.
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use github_copilot_sdk::{Client, handler::ApproveAllHandler, types::SessionConfig};
use serde::{Deserialize, Serialize};

use super::journal::Permit;
use super::models::WorkerType;

const MAX_TEXT: usize = 512_000;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    prompt: String,
    account: String,
    model: String,
    directory: PathBuf,
    owned: PathBuf,
    skills: Option<PathBuf>,
    deployment: String,
    pub(super) timeout_ms: u64,
}
impl Spec {
    pub fn capture(permit: &Permit, owned: &Path, skills: &Path) -> Result<Self, &'static str> {
        let config = &permit.job.config;
        let skill = permit.job.worker_type == WorkerType::Skill;
        if !matches!(permit.job.worker_type, WorkerType::Ai | WorkerType::Skill) {
            return Err("Choose an AI or skill worker.");
        }
        let prompt = if skill {
            let name = config
                .skill_name
                .as_deref()
                .filter(|name| !name.trim().is_empty())
                .ok_or("Choose a skill for this job.")?;
            let mut prompt = format!(
                "Use the {} skill",
                serde_json::to_string(name).map_err(|_| "The skill name is unsupported.")?
            );
            if let Some(action) = config.action.as_deref().filter(|value| !value.is_empty()) {
                prompt.push_str(&format!(
                    " to perform the {} action",
                    serde_json::to_string(action)
                        .map_err(|_| "The skill action is unsupported.")?
                ));
            }
            if let Some(params) = &config.params {
                prompt.push_str(&format!(
                    "\n\nParameters:\n{}",
                    serde_json::to_string_pretty(params)
                        .map_err(|_| "The skill parameters are unsupported.")?
                ));
            }
            prompt
        } else {
            config
                .prompt
                .clone()
                .filter(|value| !value.is_empty())
                .ok_or("Specify a prompt for this job.")?
        };
        let account = permit
            .request
            .context
            .account
            .clone()
            .ok_or("Choose a GitHub account for this job.")?;
        let model = config
            .model
            .clone()
            .or_else(|| permit.request.context.model.clone())
            .unwrap_or_else(|| "claude-sonnet-4.5".into());
        let timeout = config.timeout.unwrap_or(120_000.0);
        if !timeout.is_finite()
            || timeout.fract() != 0.0
            || !(1.0..=86_400_000.0).contains(&timeout)
        {
            return Err("Choose a timeout from 1 to 86400000 milliseconds.");
        }
        let base = std::env::current_dir().map_err(|_| "The job directory is unavailable.")?;
        let directory = if let Some(path) = config.cwd.as_deref().filter(|path| !path.is_empty()) {
            let path = PathBuf::from(path);
            if path.is_absolute() {
                path
            } else {
                base.join(path)
            }
        } else if skill {
            skills.to_owned()
        } else {
            base
        };
        let spec = Self {
            prompt,
            account,
            model,
            directory,
            owned: owned.to_owned(),
            skills: skill.then(|| skills.to_owned()),
            deployment: permit.request.deployment.clone(),
            timeout_ms: timeout as u64,
        };
        spec.validate()?;
        Ok(spec)
    }
    pub(super) fn validate(&self) -> Result<(), &'static str> {
        if !crate::gh::is_valid_slug(&self.account)
            || self.model.is_empty()
            || self.model.len() > 256
            || self.model.chars().any(char::is_control)
        {
            return Err("The captured account or model is invalid.");
        }
        if self.prompt.trim().is_empty()
            || self.prompt.len() > 256 * 1024
            || self.prompt.contains('\0')
        {
            return Err("The saved prompt exceeds 256 KiB or contains NUL.");
        }
        if !self.directory.is_absolute()
            || !self.directory.is_dir()
            || !self.owned.is_absolute()
            || self
                .skills
                .as_ref()
                .is_some_and(|path| !path.is_absolute() || !path.is_dir())
        {
            return Err("The job or skill directory is unavailable.");
        }
        if self.timeout_ms == 0
            || self.timeout_ms > 86_400_000
            || super::backend::AutomationBackend::new(&self.deployment).is_err()
        {
            return Err("The captured worker context is unsupported.");
        }
        Ok(())
    }
    fn session_config(&self) -> SessionConfig {
        let mut config =
            SessionConfig::default().with_permission_handler(Arc::new(ApproveAllHandler));
        config.model = Some(self.model.clone());
        config.allowed_models = Some(vec![self.model.clone()]);
        config.streaming = Some(true);
        config.working_directory = Some(self.directory.clone());
        config.skill_directories = self.skills.clone().map(|path| vec![path]);
        config
    }
}

fn prefix(text: &str) -> String {
    let mut end = text.len().min(MAX_TEXT);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

pub(super) fn run(spec: Spec) -> i32 {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => return 125,
    };
    runtime.block_on(run_async(spec))
}

async fn run_async(spec: Spec) -> i32 {
    let mut output = String::new();
    let outcome = execute(&spec, &mut output).await;
    let written = std::io::stdout().write_all(output.as_bytes()).is_ok();
    match outcome {
        Ok(()) if written => 0,
        result => {
            let message = result
                .err()
                .unwrap_or("The captured job output could not be delivered.");
            let _ = writeln!(std::io::stderr(), "{message}");
            126
        }
    }
}

async fn execute(spec: &Spec, output: &mut String) -> Result<(), &'static str> {
    crate::copilot_sdk::private_directory(&spec.owned)
        .map_err(|_| "The private worker directory is unavailable.")?;
    let fixture = crate::copilot_fixture::load(&spec.owned)
        .map_err(|_| "The offline worker fixture is invalid.")?;
    if let Some(fixture) = &fixture {
        fixture
            .identity(&spec.account, Some(&spec.model))
            .map_err(|_| "The offline worker identity does not match.")?;
        if !fixture.matches_backend(&spec.deployment) {
            return Err("The offline worker backend does not match.");
        }
    }
    let executable =
        std::env::current_exe().map_err(|_| "The packaged worker runtime is unavailable.")?;
    let runtime = crate::copilot_runtime::prepare(executable, spec.owned.join("runtime"))
        .await
        .map_err(|_| "The packaged worker runtime could not be prepared.")?;
    let credential = if fixture.is_some() {
        None
    } else {
        Some(
            crate::gh::auth_token(&spec.account)
                .await
                .map_err(|_| "Sign in to the captured GitHub account before running this job.")?,
        )
    };
    let turns = spec.owned.join("turns");
    crate::copilot_sdk::private_directory(&turns)
        .map_err(|_| "The private job state is unavailable.")?;
    let turn = tempfile::Builder::new()
        .prefix("job-")
        .tempdir_in(&turns)
        .map_err(|_| "The private job state is unavailable.")?;
    let mut options = crate::copilot_sdk::client_options(
        runtime.program(),
        turn.path(),
        credential,
        std::env::vars_os().map(|(name, _)| name),
    )
    .map_err(|_| "The captured worker runtime is invalid.")?;
    options.working_directory = spec.directory.clone();
    if let Some(fixture) = &fixture {
        options.request_handler = Some(fixture.clone());
        options.on_list_models = Some(fixture.clone());
    }
    let client = tokio::time::timeout(Duration::from_secs(45), Client::start(options))
        .await
        .map_err(|_| "Copilot worker startup timed out.")?
        .map_err(|_| "The Copilot worker could not start.")?;
    let outcome = turn_once(&client, spec, fixture.as_deref(), output).await;
    let stopped = matches!(
        tokio::time::timeout(Duration::from_secs(15), client.stop()).await,
        Ok(Ok(()))
    );
    if !stopped {
        client.force_stop();
    }
    let closed = turn.close().is_ok();
    if !stopped || !closed {
        let _ = writeln!(
            std::io::stderr(),
            "The worker's runtime cleanup could not be confirmed."
        );
        if outcome.is_ok() {
            return Err("The worker's runtime cleanup could not be confirmed.");
        }
    }
    outcome
}

async fn turn_once(
    client: &Client,
    spec: &Spec,
    fixture: Option<&crate::copilot_fixture::Fixture>,
    output: &mut String,
) -> Result<(), &'static str> {
    if fixture.is_none() {
        let auth = tokio::time::timeout(Duration::from_secs(10), client.get_auth_status())
            .await
            .map_err(|_| "The captured account could not be verified.")?
            .map_err(|_| "The captured account could not be verified.")?;
        if !auth.is_authenticated
            || !auth
                .login
                .as_ref()
                .is_some_and(|login| login.eq_ignore_ascii_case(&spec.account))
        {
            return Err("Copilot is not authenticated as the captured account.");
        }
    }
    let mut config = spec.session_config();
    if let Some(fixture) = fixture {
        config.provider = Some(fixture.provider());
    }
    let session = tokio::time::timeout(Duration::from_secs(30), client.create_session(config))
        .await
        .map_err(|_| "Copilot session creation timed out.")?
        .map_err(|_| "The Copilot session could not be created.")?;
    let mut execution = crate::copilot_runs::Execution::new(
        crate::copilot_runs::Context {
            account: spec.account.clone(),
            model: spec.model.clone(),
            review: None,
        },
        spec.prompt.clone(),
        false,
    )
    .map_err(|_| "The captured Copilot request is invalid.")?;
    let send = execution
        .begin_send()
        .map_err(|_| "The captured Copilot request could not start.")?;
    let mut events = session.subscribe();
    let sending = tokio::time::timeout(Duration::from_secs(30), session.send(send.prompt));
    tokio::pin!(sending);
    let mut count = 0usize;
    let mut acknowledged = false;
    let outcome = loop {
        tokio::select! {
            biased;
            event=events.recv()=>{
                let event=match event {Ok(event)=>event,Err(_)=>break Err("The worker event stream was interrupted. The task was not resent.")};
                count+=1;if count>10000 {break Err("The worker event limit was reached. The task was not resent.");}
                let action=crate::copilot_sdk::apply_event(&mut execution,send.key,&event);
                *output=prefix(execution.output());
                if execution.output().len()>MAX_TEXT {break Err("The worker output limit was reached. The captured prefix is retained.");}
                match action {
                    crate::copilot_sdk::EventAction::Finished=>break Ok(()),
                    crate::copilot_sdk::EventAction::AbortRequired=>break Err("The worker response was incomplete. The task was not resent."),
                    _=>{},
                }
            }
            response=&mut sending,if !acknowledged=>{
                if !matches!(response,Ok(Ok(_))) {break Err("The worker result is unconfirmed. The task was not resent.");}
                acknowledged=true;
            }
        }
    };
    let disconnected = matches!(
        tokio::time::timeout(Duration::from_secs(10), session.disconnect()).await,
        Ok(Ok(()))
    );
    if outcome.is_ok() && !disconnected {
        return Err("The worker session cleanup could not be confirmed.");
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::super::journal::{Context, Journal};
    use super::super::models::Claim;
    use super::*;
    use serde_json::{Value, json};

    fn permit(root: &Path, worker: &str, config: Value, account: Option<&str>) -> Permit {
        let path = root.join(format!("{}.json", uuid::Uuid::new_v4()));
        std::fs::write(&path, b"{}").unwrap();
        let mut journal = Journal::open(&path, "http://127.0.0.1:3210").unwrap();
        journal
            .prepare(
                "run-one",
                Context {
                    account: account.map(str::to_owned),
                    model: Some("captured-model".into()),
                },
            )
            .unwrap();
        let request = journal.request().unwrap();
        let claim = Claim::parse(
            json!({"kind":"claimed","requestId":request.request_id,
            "run":{"_id":"run-one","jobId":"job-one","status":"running","startedAt":1.0},
            "job":{"_id":"job-one","name":"fixture","workerType":worker,"config":config}}),
            &request.request_id,
            "run-one",
        )
        .unwrap();
        journal.start(&claim).unwrap()
    }

    #[test]
    fn skill_capture_preserves_falsy_parameters_and_exposes_only_its_captured_skill_root() {
        for params in [Value::Null, json!(false), json!(0)] {
            let root = tempfile::tempdir().unwrap();
            let permit = permit(
                root.path(),
                "skill",
                json!({"skillName":"fixture","action":"run","params":params}),
                Some("captured-account"),
            );
            let spec = Spec::capture(&permit, &root.path().join("owned"), root.path()).unwrap();
            assert!(
                spec.prompt
                    .ends_with(&serde_json::to_string_pretty(&params).unwrap())
            );
            assert_eq!(spec.account, "captured-account");
            assert_eq!(spec.model, "captured-model");
            let session = spec.session_config();
            assert_eq!(session.working_directory, Some(root.path().to_owned()));
            assert_eq!(
                session.skill_directories,
                Some(vec![root.path().to_owned()])
            );
            assert!(session.permission_handler.is_some());
            assert!(session.available_tools.is_none());
            assert_eq!(
                crate::copilot_sdk::session_config("captured-model")
                    .unwrap()
                    .available_tools,
                Some(vec![])
            );
        }
    }

    #[test]
    fn job_model_and_directory_override_defaults_without_rebinding_the_account() {
        let root = tempfile::tempdir().unwrap();
        let permit = permit(
            root.path(),
            "ai",
            json!({"prompt":"captured task","model":"job-model","cwd":root.path(),"timeout":45000.0}),
            Some("captured-account"),
        );
        let spec = Spec::capture(&permit, &root.path().join("owned"), root.path()).unwrap();
        assert_eq!(spec.prompt, "captured task");
        assert_eq!(spec.account, "captured-account");
        assert_eq!(spec.model, "job-model");
        assert_eq!(spec.directory, root.path());
        assert_eq!(spec.timeout_ms, 45000);
        assert!(spec.skills.is_none());
        assert_eq!(
            spec.session_config().allowed_models,
            Some(vec!["job-model".to_owned()])
        );
    }

    #[test]
    fn missing_identity_and_unavailable_skill_root_are_rejected_before_runtime_start() {
        let root = tempfile::tempdir().unwrap();
        let no_account = permit(root.path(), "ai", json!({"prompt":"fixture"}), None);
        assert!(Spec::capture(&no_account, &root.path().join("owned"), root.path()).is_err());
        let skill = permit(
            root.path(),
            "skill",
            json!({"skillName":"fixture"}),
            Some("captured-account"),
        );
        assert!(
            Spec::capture(
                &skill,
                &root.path().join("owned"),
                &root.path().join("missing")
            )
            .is_err()
        );
        let text = "雪".repeat(200000);
        assert_eq!(prefix(&text), "雪".repeat(MAX_TEXT / 3));
    }
}
