use super::*;
use buddy_core::automation::{
    backend::ChangeKind,
    models::{Job, MissedPolicy, Schedule, Shell, WorkerType},
};
use gpui_kit::SharedString;
use gpui_kit::component::IndexPath;
use gpui_kit::component::searchable_list::SearchableListItem;
use gpui_kit::component::select::{SelectEvent, SelectState};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// `DEFAULT_TIMEZONE` in `shared/utils/cronUtils.ts`.
pub(super) const DEFAULT_TIMEZONE: &str = "America/New_York";

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum FormKind {
    Job,
    Schedule,
}
#[derive(Clone)]
pub(super) struct Form {
    pub kind: FormKind,
    pub target: Option<String>,
    pub original: Value,
    pub worker: WorkerType,
    pub shell: Shell,
    pub policy: MissedPolicy,
    pub enabled: bool,
    pub job_id: Option<String>,
    pub fields: BTreeMap<&'static str, Entity<InputState>>,
    pub text: BTreeMap<&'static str, Entity<TextareaState>>,
    /// Searchable job picker for a new schedule; scales to long job lists.
    pub job_select: Option<Entity<SelectState<Vec<JobChoice>>>>,
}

/// A job in the schedule form's picker; names may repeat, so the value is the ID.
#[derive(Clone)]
pub(super) struct JobChoice {
    id: String,
    name: SharedString,
}
impl SearchableListItem for JobChoice {
    type Value = String;
    fn title(&self) -> SharedString {
        self.name.clone()
    }
    fn value(&self) -> &String {
        &self.id
    }
}
fn display(value: Option<&Value>) -> String {
    value
        .map(|value| match value {
            Value::String(s) => s.clone(),
            _ => value.to_string(),
        })
        .unwrap_or_default()
}
impl Form {
    pub fn new_job(
        job: Option<&Job>,
        duplicate: bool,
        window: &mut Window,
        cx: &mut Context<AutomationView>,
    ) -> Self {
        let original = job
            .and_then(|job| serde_json::to_value(job).ok())
            .unwrap_or(json!({"config":{}}));
        let mut form = Self::new(FormKind::Job, original, window, cx);
        if duplicate {
            form.target = None;
            if let Some(name) = form.fields.get("name") {
                name.update(cx, |input, cx| {
                    input.set_value(
                        format!("{} copy", job.map(|job| job.name.as_str()).unwrap_or("Job")),
                        window,
                        cx,
                    )
                });
            }
        }
        form
    }
    pub fn new_schedule(
        schedule: Option<&Schedule>,
        job_id: Option<String>,
        jobs: &[Job],
        window: &mut Window,
        cx: &mut Context<AutomationView>,
    ) -> Self {
        let original = schedule.and_then(|value| serde_json::to_value(value).ok()).unwrap_or(json!({"cron":"0 9 * * 1-5","timezone":DEFAULT_TIMEZONE,"enabled":true,"missedPolicy":"skip"}));
        let mut form = Self::new(FormKind::Schedule, original, window, cx);
        if form.job_id.is_none() {
            form.job_id = job_id;
        }
        if form.target.is_none() {
            let choices: Vec<JobChoice> = jobs
                .iter()
                .map(|job| JobChoice {
                    id: job.id.clone(),
                    name: job.name.clone().into(),
                })
                .collect();
            let selected = form
                .job_id
                .as_ref()
                .and_then(|id| choices.iter().position(|choice| &choice.id == id))
                .map(IndexPath::new);
            let select =
                cx.new(|cx| SelectState::new(choices, selected, window, cx).searchable(true));
            cx.subscribe(
                &select,
                |this, _, event: &SelectEvent<Vec<JobChoice>>, cx| {
                    if let (SelectEvent::Confirm(Some(id)), Some(form)) = (event, &mut this.form) {
                        form.job_id = Some(id.clone());
                        cx.notify();
                    }
                },
            )
            .detach();
            form.job_select = Some(select);
        }
        form
    }
    fn new(
        kind: FormKind,
        original: Value,
        window: &mut Window,
        cx: &mut Context<AutomationView>,
    ) -> Self {
        let mut fields = BTreeMap::new();
        for name in [
            "name",
            "description",
            "cwd",
            "timeout",
            "model",
            "repository",
            "skillName",
            "action",
            "cron",
            "timezone",
        ] {
            let value = if name == "repository" {
                repository(&original)
            } else if name == "timezone" && kind == FormKind::Schedule {
                // A schedule without a timezone runs in the backend default.
                original
                    .get("timezone")
                    .and_then(Value::as_str)
                    .filter(|zone| !zone.is_empty())
                    .unwrap_or(DEFAULT_TIMEZONE)
                    .to_owned()
            } else {
                display(
                    original
                        .get(name)
                        .or_else(|| original.get("config").and_then(|config| config.get(name))),
                )
            };
            let input = cx.new(|cx| InputState::new(window, cx));
            input.update(cx, |input, cx| input.set_value(value, window, cx));
            cx.subscribe(&input, |_, _, _: &InputEvent, cx| cx.notify())
                .detach();
            fields.insert(name, input);
        }
        let mut text = BTreeMap::new();
        for name in ["command", "prompt", "params", "env", "inputParams"] {
            let value = original
                .get(name)
                .or_else(|| original.get("config").and_then(|config| config.get(name)));
            let value = if matches!(name, "command" | "prompt") {
                display(value)
            } else {
                value
                    .map(|value| serde_json::to_string_pretty(value).unwrap_or_default())
                    .unwrap_or_default()
            };
            let input = cx.new(|cx| TextareaState::new(window, cx));
            input.update(cx, |input, cx| input.set_value(value, window, cx));
            cx.subscribe(&input, |_, _, _: &InputEvent, cx| cx.notify())
                .detach();
            text.insert(name, input);
        }
        Self {
            kind,
            target: original
                .get("_id")
                .and_then(Value::as_str)
                .map(str::to_owned),
            worker: original
                .get("workerType")
                .cloned()
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or(WorkerType::Exec),
            shell: original
                .pointer("/config/shell")
                .cloned()
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or(if cfg!(windows) {
                    Shell::Powershell
                } else {
                    Shell::Bash
                }),
            policy: original
                .get("missedPolicy")
                .cloned()
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or(MissedPolicy::Skip),
            enabled: original
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or(true),
            job_id: original
                .get("jobId")
                .and_then(Value::as_str)
                .map(str::to_owned),
            original,
            fields,
            text,
            job_select: None,
        }
    }
    pub fn arguments(&self, cx: &Context<AutomationView>) -> Result<(ChangeKind, Value), String> {
        let values = self
            .fields
            .iter()
            .map(|(key, input)| ((*key).to_string(), input.read(cx).value().to_string()))
            .chain(
                self.text
                    .iter()
                    .map(|(key, input)| ((*key).to_string(), input.read(cx).value().to_string())),
            )
            .collect();
        payload(
            &DraftSpec {
                kind: self.kind,
                target: self.target.as_deref(),
                original: &self.original,
                worker: self.worker,
                shell: self.shell,
                policy: self.policy,
                enabled: self.enabled,
                job_id: self.job_id.as_deref(),
            },
            &values,
        )
    }
}
/// The target repository is edited as `owner/name`, like the Electron editor.
fn repository(original: &Value) -> String {
    match (
        original
            .pointer("/config/repoOwner")
            .and_then(Value::as_str),
        original.pointer("/config/repoName").and_then(Value::as_str),
    ) {
        (Some(owner), Some(name)) => format!("{owner}/{name}"),
        _ => String::new(),
    }
}
fn parsed(values: &BTreeMap<String, String>, key: &str) -> Result<Option<Value>, String> {
    let text = values.get(key).map(String::as_str).unwrap_or("");
    if text.trim().is_empty() {
        return Ok(None);
    }
    serde_json::from_str(text)
        .map(Some)
        .map_err(|_| format!("{key} must contain valid JSON, or be empty."))
}
pub(super) struct DraftSpec<'a> {
    pub kind: FormKind,
    pub target: Option<&'a str>,
    pub original: &'a Value,
    pub worker: WorkerType,
    pub shell: Shell,
    pub policy: MissedPolicy,
    pub enabled: bool,
    pub job_id: Option<&'a str>,
}
pub(super) fn payload(
    spec: &DraftSpec<'_>,
    values: &BTreeMap<String, String>,
) -> Result<(ChangeKind, Value), String> {
    let DraftSpec {
        kind,
        target,
        original,
        worker,
        shell,
        policy,
        enabled,
        job_id,
    } = *spec;
    let get = |key: &str| values.get(key).cloned().unwrap_or_default();
    let name = get("name");
    if name.trim().is_empty() || name.len() > 256 {
        return Err("Enter a name of 1 to 256 bytes.".into());
    }
    let mut args = json!({"name":name,"description":get("description")});
    if let Some(id) = target {
        args["id"] = id.into();
    }
    if kind == FormKind::Schedule {
        let cron = get("cron");
        let timezone = get("timezone");
        if cron.trim().is_empty() || timezone.trim().is_empty() {
            return Err("Enter a cron expression and IANA timezone.".into());
        }
        if target.is_none() {
            args["jobId"] = job_id
                .filter(|id| !id.is_empty())
                .ok_or("Choose a job for this schedule.")?
                .into();
        }
        args["cron"] = cron.into();
        args["timezone"] = timezone.into();
        args["enabled"] = enabled.into();
        args["missedPolicy"] = json!(policy);
        if let Some(params) = parsed(values, "params")? {
            args["params"] = params;
        } else if target.is_some() && original.get("params").is_some() {
            // schedules:update copies only defined fields; null clears stored parameters.
            args["params"] = Value::Null;
        }
        return Ok((
            if target.is_some() {
                ChangeKind::UpdateSchedule
            } else {
                ChangeKind::CreateSchedule
            },
            args,
        ));
    }
    let mut config = original
        .get("config")
        .filter(|v| v.is_object())
        .cloned()
        .unwrap_or(json!({}));
    for field in ["command", "prompt", "cwd", "model", "skillName", "action"] {
        let text = get(field);
        if text.is_empty() {
            config
                .as_object_mut()
                .ok_or("The original configuration is invalid.")?
                .remove(field);
        } else {
            config[field] = text.into();
        }
    }
    let timeout = get("timeout");
    if timeout.trim().is_empty() {
        config
            .as_object_mut()
            .ok_or("The original configuration is invalid.")?
            .remove("timeout");
    } else {
        let number: f64 = timeout
            .parse()
            .map_err(|_| "Timeout must be a number of milliseconds.")?;
        if !number.is_finite() || number.fract() != 0.0 || !(1.0..=86_400_000.0).contains(&number) {
            return Err("Timeout must be 1 to 86400000 milliseconds.".into());
        }
        config["timeout"] = json!(number);
    }
    for field in ["params", "env"] {
        if let Some(value) = parsed(values, field)? {
            if field == "env"
                && value.as_object().is_none_or(|object| {
                    object.iter().any(|(key, value)| {
                        key.is_empty()
                            || key.contains(['=', '\0'])
                            || !value.is_string()
                            || value.as_str().is_some_and(|v| v.contains('\0'))
                    })
                })
            {
                return Err(
                    "Environment must be a JSON object of variable names and string values.".into(),
                );
            }
            // The command worker's own limits; a larger environment would only
            // fail later, after a run was claimed.
            if field == "env"
                && value.as_object().is_some_and(|object| {
                    object.len() > 1024
                        || object
                            .iter()
                            .map(|(key, value)| key.len() + value.as_str().map_or(0, str::len))
                            .sum::<usize>()
                            > 64 * 1024
                })
            {
                return Err(
                    "Environment allows at most 1024 variables and 65536 bytes of names and values."
                        .into(),
                );
            }
            config[field] = value;
        } else if field == "env" && config.get("env").is_some() {
            // jobs:update keeps an omitted env for older editors; clear it explicitly.
            config["env"] = json!({});
        } else {
            config
                .as_object_mut()
                .ok_or("The original configuration is invalid.")?
                .remove(field);
        }
    }
    let repository = get("repository");
    let object = config
        .as_object_mut()
        .ok_or("The original configuration is invalid.")?;
    match repository.trim().split_once('/') {
        _ if repository.trim().is_empty() => {
            object.remove("repoOwner");
            object.remove("repoName");
        }
        Some((owner, name)) if !owner.is_empty() && !name.is_empty() && !name.contains('/') => {
            object.insert("repoOwner".into(), owner.into());
            object.insert("repoName".into(), name.into());
        }
        _ => return Err("Target repository must be owner/name, or empty.".into()),
    }
    if worker == WorkerType::Exec {
        config["shell"] = json!(shell);
    }
    let required = match worker {
        WorkerType::Exec => "command",
        WorkerType::Ai => "prompt",
        WorkerType::Skill => "skillName",
    };
    if config
        .get(required)
        .and_then(Value::as_str)
        .is_none_or(|value| value.trim().is_empty())
    {
        return Err(format!("Enter {required} for this worker."));
    }
    args["workerType"] = json!(worker);
    args["config"] = config;
    if let Some(parameters) = parsed(values, "inputParams")? {
        if !parameters.is_array() {
            return Err("Input parameters must be a JSON array.".into());
        }
        args["inputParams"] = parameters;
    } else if target.is_some() && original.get("inputParams").is_some() {
        // jobs:update keeps an omitted field; an empty list clears the definitions.
        args["inputParams"] = json!([]);
    }
    Ok((
        if target.is_some() {
            ChangeKind::UpdateJob
        } else {
            ChangeKind::CreateJob
        },
        args,
    ))
}
