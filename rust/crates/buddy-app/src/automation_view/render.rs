//! Jobs, schedules and run history. Recovery states lock editing until resolved.
use buddy_core::automation::{
    engine::Phase,
    models::{Job, MissedPolicy, Run, Schedule, Shell, WorkerType},
};
use gpui_kit::{AnyElement, App, SharedString};

use super::*;

pub(super) const PAGE: usize = 8;
/// Long outputs show their end, where results and errors usually are.
const OUTPUT_LIMIT: usize = 16 * 1024;

pub(super) fn phase_label(phase: Phase) -> &'static str {
    match phase {
        Phase::Idle => "Dispatcher idle",
        Phase::Claiming => "Claiming a queued run",
        Phase::Running => "Running a claimed job",
        Phase::Reconciling => "Reporting a finished run",
        Phase::Attention => "Dispatcher needs attention",
    }
}

pub(super) fn status_label(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Pending => "Queued",
        RunStatus::Running => "Running",
        RunStatus::Completed => "Completed",
        RunStatus::Failed => "Failed",
        RunStatus::Cancelled => "Cancelled",
    }
}

pub(super) fn worker_label(worker: WorkerType) -> &'static str {
    match worker {
        WorkerType::Exec => "Shell command",
        WorkerType::Ai => "AI prompt",
        WorkerType::Skill => "Skill",
    }
}

pub(super) fn policy_label(policy: MissedPolicy) -> &'static str {
    match policy {
        MissedPolicy::Skip => "Skip missed runs",
        MissedPolicy::Last => "Run the latest missed",
        MissedPolicy::Catchup => "Run every missed",
    }
}

fn shell_label(shell: Shell) -> &'static str {
    match shell {
        Shell::Powershell => "PowerShell",
        Shell::Bash => "Bash",
        Shell::Cmd => "Command Prompt",
    }
}

/// Convex stores milliseconds since the epoch; show local wall time.
pub(super) fn local_time(milliseconds: f64) -> String {
    chrono::DateTime::from_timestamp_millis(milliseconds as i64)
        .map(|time| {
            time.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|| "Unknown time".into())
}

/// The last `OUTPUT_LIMIT` bytes, cut on a character boundary.
pub(super) fn bounded_output(text: &str) -> (String, bool) {
    if text.len() <= OUTPUT_LIMIT {
        return (text.to_owned(), false);
    }
    let mut start = text.len() - OUTPUT_LIMIT;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    (text[start..].to_owned(), true)
}

/// Standard output first, then standard error under its own heading.
pub(super) fn combined_output(stdout: &str, stderr: &str) -> String {
    match (stdout.is_empty(), stderr.is_empty()) {
        (_, true) => stdout.to_owned(),
        (true, false) => format!(
            "Standard error:
{stderr}"
        ),
        (false, false) => format!(
            "{stdout}

Standard error:
{stderr}"
        ),
    }
}

/// Live `(stdout, stderr)` belongs to the run whose permit started the worker.
pub(super) fn live_output(
    run: Option<String>,
    output: Option<(String, String)>,
) -> Option<(String, String)> {
    run.zip(output)
        .map(|(run, (stdout, stderr))| (run, combined_output(&stdout, &stderr)))
}

/// Command workers store `{stdout, stderr, …}`; other workers store text or JSON.
pub(super) fn run_output(run: &Run) -> Option<String> {
    run.output.as_ref().map(|value| match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        Value::Object(fields)
            if fields.get("stdout").is_some_and(Value::is_string)
                || fields.get("stderr").is_some_and(Value::is_string) =>
        {
            let text = |key| fields.get(key).and_then(Value::as_str).unwrap_or("");
            combined_output(text("stdout"), text("stderr"))
        }
        other => serde_json::to_string_pretty(other).unwrap_or_default(),
    })
}

/// One line describing what a job does.
pub(super) fn job_summary(job: &Job) -> String {
    let text = match job.worker_type {
        WorkerType::Exec => job.config.command.clone().unwrap_or_default(),
        WorkerType::Ai => job.config.prompt.clone().unwrap_or_default(),
        WorkerType::Skill => match &job.config.action {
            Some(action) => format!(
                "{} · {action}",
                job.config.skill_name.as_deref().unwrap_or("")
            ),
            None => job.config.skill_name.clone().unwrap_or_default(),
        },
    };
    let line = text.lines().next().unwrap_or("").trim();
    let mut summary: String = line.chars().take(120).collect();
    if line.chars().count() > 120 || text.lines().nth(1).is_some() {
        summary.push('…');
    }
    summary
}

fn page_bounds(count: usize, page: usize) -> usize {
    page.min(count.saturating_sub(1) / PAGE)
}

impl AutomationView {
    fn job_name<'a>(&'a self, id: &str) -> &'a str {
        self.snapshot
            .jobs
            .iter()
            .find(|job| job.id == id)
            .map(|job| job.name.as_str())
            .unwrap_or("Deleted job")
    }

    fn select_pane(&mut self, pane: Pane, cx: &mut Context<Self>) {
        self.pane = pane;
        self.page = 0;
        self.job_page = 0;
        cx.notify();
    }

    fn open_job(
        &mut self,
        job: Option<Job>,
        duplicate: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.form = Some(Form::new_job(job.as_ref(), duplicate, window, cx));
        self.error = None;
        cx.notify();
    }

    fn open_schedule(
        &mut self,
        schedule: Option<Schedule>,
        job_id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.form = Some(Form::new_schedule(schedule.as_ref(), job_id, window, cx));
        self.error = None;
        cx.notify();
    }

    fn render_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut tabs = h_flex().flex_wrap().gap_2();
        for (index, (label, pane)) in [
            ("Jobs", Pane::Jobs),
            ("Schedules", Pane::Schedules),
            ("History", Pane::History),
        ]
        .into_iter()
        .enumerate()
        {
            tabs = tabs.child(
                Button::new(("automation-pane", index))
                    .label(label)
                    .selected(self.pane == pane)
                    .on_click(cx.listener(move |this, _, _, cx| this.select_pane(pane, cx))),
            );
        }
        let editing = self.form.is_some();
        h_flex()
            .flex_wrap()
            .gap_3()
            .debug_selector(|| "automation-primary-controls".into())
            .child(div().text_xl().child("Automation"))
            .child(tabs.flex_shrink_0())
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        Button::new("automation-refresh")
                            .label("Refresh")
                            .disabled(self.busy || self.handle.is_none())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.operate(Command::Refresh, false, cx)
                            })),
                    )
                    .child(
                        Button::new("automation-new-job")
                            .label("New job")
                            .disabled(self.locked() || editing || !self.snapshot.loaded)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_job(None, false, window, cx)
                            })),
                    )
                    .child(
                        Button::new("automation-new-schedule")
                            .label("New schedule")
                            .disabled(
                                self.locked()
                                    || editing
                                    || !self.snapshot.loaded
                                    || self.snapshot.jobs.is_empty(),
                            )
                            .on_click(cx.listener(|this, _, window, cx| {
                                let job = this.snapshot.jobs.first().map(|job| job.id.clone());
                                this.open_schedule(None, job, window, cx)
                            })),
                    ),
            )
            .into_any_element()
    }

    fn render_status(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let snapshot = &self.snapshot;
        let connection = if self.handle.is_none() {
            "Not started"
        } else if snapshot.connected {
            "Connected"
        } else if snapshot.loaded {
            "Offline · showing the last loaded data"
        } else if snapshot.connection_error.is_some() {
            "Offline"
        } else {
            "Connecting…"
        };
        let mut status = v_flex().gap_2().child(
            div()
                .whitespace_normal()
                .text_color(theme.muted_foreground)
                .child(format!("{connection} · {}", phase_label(snapshot.phase))),
        );
        if let Some(notice) = &snapshot.notice {
            status = status.child(div().whitespace_normal().child(notice.clone()));
        }
        let mut errors: Vec<&String> = Vec::new();
        for error in [
            snapshot.connection_error.as_ref(),
            snapshot.dispatch_error.as_ref(),
            snapshot.error.as_ref(),
            self.error.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            // A failed command reports the same text through its reply.
            if !errors.contains(&error) {
                errors.push(error);
            }
        }
        for error in errors {
            status = status.child(
                div()
                    .whitespace_normal()
                    .text_color(theme.danger)
                    .child(error.clone()),
            );
        }
        if let Some(edit) = &snapshot.edit {
            status = status.child(self.render_edit_recovery(edit.kind, cx));
        }
        if let Some(trigger) = &snapshot.trigger {
            let job = self.job_name(&trigger.job_id).to_owned();
            status = status.child(self.render_trigger_recovery(job, cx));
        }
        status.into_any_element()
    }

    fn render_edit_recovery(&self, kind: ChangeKind, cx: &mut Context<Self>) -> AnyElement {
        let acknowledged = self.snapshot.edit_acknowledged;
        let action = match kind {
            ChangeKind::CreateJob => "Create job",
            ChangeKind::UpdateJob => "Update job",
            ChangeKind::DeleteJob => "Delete job",
            ChangeKind::CreateSchedule => "Create schedule",
            ChangeKind::UpdateSchedule => "Update schedule",
            ChangeKind::DeleteSchedule => "Delete schedule",
            ChangeKind::CancelRun => "Cancel run",
        };
        let message = if acknowledged {
            format!("{action} was confirmed. Finish local cleanup before the next change.")
        } else {
            format!(
                "{action} has an unknown outcome. It may already have been applied, so it is never sent again. Refresh to check, then discard the local copy."
            )
        };
        let mut buttons = h_flex().flex_wrap().gap_2();
        if acknowledged {
            buttons = buttons.child(
                Button::new("automation-edit-cleanup")
                    .label("Finish cleanup")
                    .disabled(self.busy)
                    .on_click(
                        cx.listener(|this, _, _, cx| this.operate(Command::ClearEdit, false, cx)),
                    ),
            );
        } else if self.discard_confirm {
            buttons = buttons
                .child(
                    Button::new("automation-edit-discard-confirm")
                        .label("Discard local copy")
                        .disabled(self.busy)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.discard_confirm = false;
                            this.operate(Command::DiscardEdit, false, cx)
                        })),
                )
                .child(
                    Button::new("automation-edit-discard-cancel")
                        .label("Keep it")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.discard_confirm = false;
                            cx.notify();
                        })),
                );
        } else {
            buttons = buttons.child(
                Button::new("automation-edit-discard")
                    .label("Discard…")
                    .disabled(self.busy)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.discard_confirm = true;
                        cx.notify();
                    })),
            );
        }
        v_flex()
            .gap_2()
            .p_3()
            .border_1()
            .rounded_md()
            .border_color(cx.theme().warning)
            .debug_selector(|| "automation-edit-recovery".into())
            .child(div().whitespace_normal().child(message))
            .child(buttons)
            .into_any_element()
    }

    fn render_trigger_recovery(&self, job: String, cx: &mut Context<Self>) -> AnyElement {
        let receipt = self.snapshot.trigger_receipt.clone();
        let message = match &receipt {
            Some(receipt) => format!(
                "The run request for {job} is confirmed as run {}. Dismiss it to queue another.",
                receipt.run_id
            ),
            None => format!(
                "A run request for {job} has an unknown outcome or was rejected. Check again, retry the same request (the server accepts it at most once), or discard it."
            ),
        };
        let mut buttons = h_flex().flex_wrap().gap_2();
        if receipt.is_some() {
            buttons =
                buttons.child(
                    Button::new("automation-trigger-clear")
                        .label("Dismiss")
                        .disabled(self.busy)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.operate(Command::ClearTrigger, false, cx)
                        })),
                );
        } else {
            buttons = buttons
                .child(
                    Button::new("automation-trigger-check")
                        .label("Check again")
                        .disabled(self.busy)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.operate(Command::ReconcileTrigger, false, cx)
                        })),
                )
                .child(
                    Button::new("automation-trigger-retry")
                        .label("Retry same request")
                        .disabled(self.busy)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.operate(Command::RetryTrigger, false, cx)
                        })),
                );
            buttons = if self.trigger_discard_confirm {
                buttons
                    .child(
                        Button::new("automation-trigger-discard-confirm")
                            .label("Discard request")
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.trigger_discard_confirm = false;
                                this.operate(Command::DiscardTrigger, false, cx)
                            })),
                    )
                    .child(
                        Button::new("automation-trigger-discard-cancel")
                            .label("Keep request")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.trigger_discard_confirm = false;
                                cx.notify();
                            })),
                    )
            } else {
                buttons.child(
                    Button::new("automation-trigger-discard")
                        .label("Discard…")
                        .disabled(self.busy)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.trigger_discard_confirm = true;
                            cx.notify();
                        })),
                )
            };
        }
        v_flex()
            .gap_2()
            .p_3()
            .border_1()
            .rounded_md()
            .border_color(cx.theme().warning)
            .debug_selector(|| "automation-trigger-recovery".into())
            .child(div().whitespace_normal().child(message))
            .child(buttons)
            .into_any_element()
    }

    fn labeled(label: &'static str, field: impl IntoElement) -> gpui_kit::Div {
        v_flex().gap_1().child(div().child(label)).child(field)
    }

    fn choice<T: Copy + PartialEq + 'static>(
        id: &'static str,
        options: &[(T, &'static str)],
        current: T,
        disabled: bool,
        cx: &mut Context<Self>,
        set: impl Fn(&mut Form, T) + Copy + 'static,
    ) -> gpui_kit::Div {
        let mut row = h_flex().flex_wrap().gap_2();
        for (index, (value, label)) in options.iter().copied().enumerate() {
            row = row.child(
                Button::new((id, index))
                    .label(label)
                    .selected(current == value)
                    .disabled(disabled)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(form) = &mut this.form {
                            set(form, value);
                        }
                        cx.notify();
                    })),
            );
        }
        row
    }

    fn render_form(&self, form: &Form, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.busy;
        let input = |name: &str| Input::new(&form.fields[name]).disabled(busy);
        let text = |name: &str| Textarea::new(&form.text[name]).disabled(busy);
        let creating = form.target.is_none();
        let title = match (form.kind, creating) {
            (FormKind::Job, true) => "New job",
            (FormKind::Job, false) => "Edit job",
            (FormKind::Schedule, true) => "New schedule",
            (FormKind::Schedule, false) => "Edit schedule",
        };
        let mut body = v_flex()
            .gap_3()
            .p_3()
            .border_1()
            .rounded_md()
            .border_color(cx.theme().border)
            .debug_selector(|| "automation-editor".into())
            .child(div().text_lg().child(title))
            .child(Self::labeled("Name", input("name")))
            .child(Self::labeled("Description", input("description")));
        match form.kind {
            FormKind::Job => {
                body = body.child(Self::labeled(
                    "Worker",
                    Self::choice(
                        "automation-worker",
                        &[
                            (WorkerType::Exec, worker_label(WorkerType::Exec)),
                            (WorkerType::Ai, worker_label(WorkerType::Ai)),
                            (WorkerType::Skill, worker_label(WorkerType::Skill)),
                        ],
                        form.worker,
                        busy,
                        cx,
                        |form, value| form.worker = value,
                    ),
                ));
                body = match form.worker {
                    WorkerType::Exec => body
                        .child(Self::labeled("Command", text("command")))
                        .child(Self::labeled(
                            "Shell",
                            Self::choice(
                                "automation-shell",
                                &[
                                    (Shell::Powershell, shell_label(Shell::Powershell)),
                                    (Shell::Bash, shell_label(Shell::Bash)),
                                    (Shell::Cmd, shell_label(Shell::Cmd)),
                                ],
                                form.shell,
                                busy,
                                cx,
                                |form, value| form.shell = value,
                            ),
                        ))
                        .child(Self::labeled("Working directory (optional)", input("cwd")))
                        .child(Self::labeled(
                            "Timeout in milliseconds (optional)",
                            input("timeout"),
                        ))
                        .child(Self::labeled(
                            "Environment (JSON object of names and string values, optional)",
                            text("env"),
                        )),
                    WorkerType::Ai => body
                        .child(Self::labeled("Prompt", text("prompt")))
                        .child(Self::labeled("Model (optional)", input("model")))
                        .child(Self::labeled(
                            "Target repository (owner/name, optional)",
                            input("repository"),
                        ))
                        .child(Self::labeled(
                            "Timeout in milliseconds (optional)",
                            input("timeout"),
                        )),
                    WorkerType::Skill => body
                        .child(Self::labeled("Skill name", input("skillName")))
                        .child(Self::labeled("Action (optional)", input("action")))
                        .child(Self::labeled("Parameters (JSON, optional)", text("params")))
                        .child(Self::labeled(
                            "Timeout in milliseconds (optional)",
                            input("timeout"),
                        )),
                };
                body = body.child(Self::labeled(
                    "Input parameters (JSON array, optional)",
                    text("inputParams"),
                ));
            }
            FormKind::Schedule => {
                if creating {
                    let jobs: Vec<(String, String)> = self
                        .snapshot
                        .jobs
                        .iter()
                        .map(|job| (job.id.clone(), job.name.clone()))
                        .collect();
                    let mut row = h_flex().flex_wrap().gap_2();
                    for (index, (id, name)) in jobs.into_iter().enumerate() {
                        let selected = form.job_id.as_deref() == Some(id.as_str());
                        row = row.child(
                            Button::new(("automation-schedule-job", index))
                                .label(name)
                                .selected(selected)
                                .disabled(busy)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(form) = &mut this.form {
                                        form.job_id = Some(id.clone());
                                    }
                                    cx.notify();
                                })),
                        );
                    }
                    body = body.child(Self::labeled("Job", row));
                } else {
                    let job = form
                        .job_id
                        .as_deref()
                        .map(|id| self.job_name(id))
                        .unwrap_or("Deleted job");
                    body = body.child(Self::labeled("Job", div().child(job.to_owned())));
                }
                body = body
                    .child(Self::labeled(
                        "Cron (minute hour day month weekday)",
                        input("cron"),
                    ))
                    .child(Self::labeled("Timezone (IANA name)", input("timezone")))
                    .child(Self::labeled(
                        "When missed",
                        Self::choice(
                            "automation-policy",
                            &[
                                (MissedPolicy::Skip, policy_label(MissedPolicy::Skip)),
                                (MissedPolicy::Last, policy_label(MissedPolicy::Last)),
                                (MissedPolicy::Catchup, policy_label(MissedPolicy::Catchup)),
                            ],
                            form.policy,
                            busy,
                            cx,
                            |form, value| form.policy = value,
                        ),
                    ))
                    .child(Self::labeled(
                        "State",
                        Self::choice(
                            "automation-enabled",
                            &[(true, "Enabled"), (false, "Paused")],
                            form.enabled,
                            busy,
                            cx,
                            |form, value| form.enabled = value,
                        ),
                    ))
                    .child(Self::labeled("Parameters (JSON, optional)", text("params")));
            }
        }
        body.child(
            h_flex()
                .flex_wrap()
                .gap_2()
                .child(
                    Button::new("automation-save")
                        .label("Save")
                        .disabled(self.locked())
                        .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                )
                .child(
                    Button::new("automation-cancel-edit")
                        .label("Close")
                        .disabled(busy)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.form = None;
                            this.error = None;
                            cx.notify();
                        })),
                ),
        )
        .into_any_element()
    }

    fn render_confirmations(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if let Some((kind, _, name)) = &self.deletion {
            let (what, confirm) = if *kind == ChangeKind::DeleteJob {
                (
                    format!("{name} and its schedules will be removed."),
                    "Delete job",
                )
            } else {
                (format!("{name} will be removed."), "Delete schedule")
            };
            return Some(
                v_flex()
                    .gap_2()
                    .p_3()
                    .border_1()
                    .rounded_md()
                    .border_color(cx.theme().danger)
                    .debug_selector(|| "automation-delete-confirmation".into())
                    .child(div().whitespace_normal().child(what))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("automation-delete-confirm")
                                    .label(confirm)
                                    .disabled(self.locked())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if let Some((kind, id, _)) = this.deletion.clone() {
                                            this.operate(
                                                Command::Save(kind, serde_json::json!({"id": id})),
                                                true,
                                                cx,
                                            );
                                        }
                                    })),
                            )
                            .child(
                                Button::new("automation-delete-cancel")
                                    .label("Keep")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.deletion = None;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .into_any_element(),
            );
        }
        let (_, name) = self.run_target.as_ref()?;
        Some(
            v_flex()
                .gap_2()
                .p_3()
                .border_1()
                .rounded_md()
                .border_color(cx.theme().border)
                .debug_selector(|| "automation-run-input".into())
                .child(div().whitespace_normal().child(format!("Run {name} now")))
                .child(Textarea::new(&self.run_input).disabled(self.busy))
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("automation-run-confirm")
                                .label("Queue run")
                                .disabled(self.locked())
                                .on_click(cx.listener(|this, _, _, cx| this.queue_run(cx))),
                        )
                        .child(
                            Button::new("automation-run-cancel")
                                .label("Close")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.run_target = None;
                                    cx.notify();
                                })),
                        ),
                )
                .into_any_element(),
        )
    }

    fn pager(
        &self,
        id: &'static str,
        count: usize,
        page: usize,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if count <= PAGE {
            return None;
        }
        let pages = count.div_ceil(PAGE);
        let jobs = id == "automation-jobs";
        Some(
            h_flex()
                .gap_2()
                .items_center()
                .child(
                    Button::new((id, 0usize))
                        .label("Previous")
                        .disabled(page == 0)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let target = if jobs {
                                &mut this.job_page
                            } else {
                                &mut this.page
                            };
                            *target = page.saturating_sub(1);
                            cx.notify();
                        })),
                )
                .child(div().child(format!("Page {} of {pages}", page + 1)))
                .child(
                    Button::new((id, 1usize))
                        .label("Next")
                        .disabled(page + 1 >= pages)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let target = if jobs {
                                &mut this.job_page
                            } else {
                                &mut this.page
                            };
                            *target = page + 1;
                            cx.notify();
                        })),
                )
                .into_any_element(),
        )
    }

    fn row(index: usize, prefix: &'static str, cx: &App) -> gpui_kit::Div {
        v_flex()
            .debug_selector(move || format!("{prefix}-{index}"))
            .gap_1()
            .pb_3()
            .border_b_1()
            .border_color(cx.theme().border)
    }

    fn render_jobs(&self, cx: &mut Context<Self>) -> AnyElement {
        let jobs: Vec<Job> = self
            .snapshot
            .jobs
            .iter()
            .filter(|job| {
                self.matches(&job.name, cx)
                    || self.matches(job.description.as_deref().unwrap_or(""), cx)
                    || self.matches(&job_summary(job), cx)
            })
            .cloned()
            .collect();
        let page = page_bounds(jobs.len(), self.job_page);
        let mut list = v_flex().gap_3();
        if jobs.is_empty() {
            list = list.child(
                div()
                    .whitespace_normal()
                    .child(if self.snapshot.jobs.is_empty() {
                        "No jobs yet. Create one to run commands, prompts or skills."
                    } else {
                        "No jobs match the filter."
                    }),
            );
        }
        let locked = self.locked() || self.form.is_some();
        for (index, job) in jobs.iter().skip(page * PAGE).take(PAGE).enumerate() {
            let schedules = self
                .snapshot
                .schedules
                .iter()
                .filter(|schedule| schedule.job_id == job.id)
                .count();
            let (run, edit, copy, schedule, delete) = (
                job.clone(),
                job.clone(),
                job.clone(),
                job.id.clone(),
                job.clone(),
            );
            let mut row = Self::row(index, "automation-job-row", cx)
                .child(div().whitespace_normal().child(job.name.clone()))
                .child(
                    div()
                        .whitespace_normal()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!(
                            "{} · {} · {schedules} schedule{}",
                            worker_label(job.worker_type),
                            job_summary(job),
                            if schedules == 1 { "" } else { "s" }
                        )),
                );
            if let Some(description) = job.description.as_ref().filter(|text| !text.is_empty()) {
                row = row.child(div().whitespace_normal().child(description.clone()));
            }
            row = row.child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        Button::new(("automation-job-run", index))
                            .label("Run now")
                            .disabled(locked)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.run_target = Some((run.id.clone(), run.name.clone()));
                                this.run_input
                                    .update(cx, |input, cx| input.set_value("", window, cx));
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new(("automation-job-edit", index))
                            .label("Edit")
                            .disabled(locked)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_job(Some(edit.clone()), false, window, cx)
                            })),
                    )
                    .child(
                        Button::new(("automation-job-duplicate", index))
                            .label("Duplicate")
                            .disabled(locked)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_job(Some(copy.clone()), true, window, cx)
                            })),
                    )
                    .child(
                        Button::new(("automation-job-schedule", index))
                            .label("Schedule")
                            .disabled(locked)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_schedule(None, Some(schedule.clone()), window, cx)
                            })),
                    )
                    .child(
                        Button::new(("automation-job-delete", index))
                            .label("Delete")
                            .disabled(locked)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.deletion = Some((
                                    ChangeKind::DeleteJob,
                                    delete.id.clone(),
                                    delete.name.clone(),
                                ));
                                cx.notify();
                            })),
                    ),
            );
            list = list.child(row);
        }
        list.children(self.pager("automation-jobs", jobs.len(), page, cx))
            .into_any_element()
    }

    fn render_schedules(&self, cx: &mut Context<Self>) -> AnyElement {
        let schedules: Vec<Schedule> = self
            .snapshot
            .schedules
            .iter()
            .filter(|schedule| {
                self.matches(&schedule.name, cx)
                    || self.matches(&schedule.cron, cx)
                    || self.matches(self.job_name(&schedule.job_id), cx)
            })
            .cloned()
            .collect();
        let page = page_bounds(schedules.len(), self.page);
        let mut list = v_flex().gap_3();
        if schedules.is_empty() {
            list = list.child(div().whitespace_normal().child(
                if self.snapshot.schedules.is_empty() {
                    "No schedules. Schedule a job to run it automatically."
                } else {
                    "No schedules match the filter."
                },
            ));
        }
        let locked = self.locked() || self.form.is_some();
        for (index, schedule) in schedules.iter().skip(page * PAGE).take(PAGE).enumerate() {
            let next = schedule
                .extra
                .get("nextRunAt")
                .and_then(Value::as_f64)
                .map(local_time);
            let last = schedule
                .extra
                .get("lastRunAt")
                .and_then(Value::as_f64)
                .map(local_time);
            let (edit, toggle, delete) = (schedule.clone(), schedule.clone(), schedule.clone());
            let row = Self::row(index, "automation-schedule-row", cx)
                .child(div().whitespace_normal().child(format!(
                    "{} · {}",
                    schedule.name,
                    if schedule.enabled { "Enabled" } else { "Paused" }
                )))
                .child(
                    div()
                        .whitespace_normal()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!(
                            "{} · {} {} · {}",
                            self.job_name(&schedule.job_id),
                            schedule.cron,
                            schedule.timezone.as_deref().unwrap_or("America/New_York"),
                            policy_label(schedule.missed_policy)
                        )),
                )
                .child(div().whitespace_normal().child(format!(
                    "Next: {} · Last: {}",
                    next.unwrap_or_else(|| "Not scheduled".into()),
                    last.unwrap_or_else(|| "Never".into())
                )))
                .child(
                    h_flex()
                        .flex_wrap()
                        .gap_2()
                        .child(
                            Button::new(("automation-schedule-edit", index))
                                .label("Edit")
                                .disabled(locked)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.open_schedule(Some(edit.clone()), None, window, cx)
                                })),
                        )
                        .child(
                            Button::new(("automation-schedule-toggle", index))
                                .label(if schedule.enabled { "Pause" } else { "Enable" })
                                .disabled(locked)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    // An explicit target state, never a retryable toggle.
                                    this.operate(
                                        Command::Save(
                                            ChangeKind::UpdateSchedule,
                                            serde_json::json!({"id": toggle.id, "enabled": !toggle.enabled}),
                                        ),
                                        false,
                                        cx,
                                    )
                                })),
                        )
                        .child(
                            Button::new(("automation-schedule-delete", index))
                                .label("Delete")
                                .disabled(locked)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.deletion = Some((
                                        ChangeKind::DeleteSchedule,
                                        delete.id.clone(),
                                        delete.name.clone(),
                                    ));
                                    cx.notify();
                                })),
                        ),
                );
            list = list.child(row);
        }
        list.children(self.pager("automation-schedules", schedules.len(), page, cx))
            .into_any_element()
    }

    fn render_history(&self, cx: &mut Context<Self>) -> AnyElement {
        let runs: Vec<Run> = self
            .snapshot
            .runs
            .iter()
            .filter(|run| {
                self.matches(self.job_name(&run.job_id), cx)
                    || self.matches(status_label(run.status), cx)
            })
            .cloned()
            .collect();
        let page = page_bounds(runs.len(), self.page);
        let mut list = v_flex().gap_3();
        if runs.is_empty() {
            list = list.child(div().child(if self.snapshot.runs.is_empty() {
                "No runs yet."
            } else {
                "No runs match the filter."
            }));
        }
        for (index, run) in runs.iter().skip(page * PAGE).take(PAGE).enumerate() {
            let selected = self.selected_run.as_deref() == Some(run.id.as_str());
            let trigger = match run.extra.get("triggeredBy").and_then(Value::as_str) {
                Some("schedule") => "Scheduled",
                Some("api") => "API",
                _ if run.schedule_id.is_some() => "Scheduled",
                _ => "Manual",
            };
            let duration = run
                .extra
                .get("duration")
                .and_then(Value::as_f64)
                .map(|ms| format!(" · {:.1} s", ms / 1000.0))
                .unwrap_or_default();
            let (toggle, cancel) = (run.id.clone(), run.id.clone());
            let status_color = match run.status {
                RunStatus::Failed => cx.theme().danger,
                RunStatus::Completed => cx.theme().success,
                _ => cx.theme().foreground,
            };
            let mut buttons = h_flex().flex_wrap().gap_2().child(
                Button::new(("automation-run-details", index))
                    .label(if selected {
                        "Hide output"
                    } else {
                        "Show output"
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected_run = (this.selected_run.as_deref() != Some(toggle.as_str()))
                            .then(|| toggle.clone());
                        cx.notify();
                    })),
            );
            if run.status.is_active() {
                buttons = buttons.child(
                    Button::new(("automation-run-cancel", index))
                        .label("Cancel run")
                        .disabled(self.locked())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.operate(Command::Cancel(cancel.clone()), false, cx)
                        })),
                );
            }
            let mut row = Self::row(index, "automation-run-row", cx)
                .child(
                    h_flex()
                        .flex_wrap()
                        .gap_2()
                        .child(
                            div()
                                .text_color(status_color)
                                .child(status_label(run.status)),
                        )
                        .child(div().child(self.job_name(&run.job_id).to_owned())),
                )
                .child(
                    div()
                        .whitespace_normal()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!(
                            "{trigger} · started {}{duration}",
                            local_time(run.started_at)
                        )),
                );
            if let Some(error) = &run.error {
                row = row.child(
                    div()
                        .whitespace_normal()
                        .text_color(cx.theme().danger)
                        .child(error.clone()),
                );
            }
            row = row.child(buttons);
            if selected {
                let live = self
                    .snapshot
                    .output
                    .as_ref()
                    .filter(|(id, _)| id == &run.id)
                    .map(|(_, text)| text.clone());
                let text = live.or_else(|| run_output(run)).unwrap_or_default();
                let (shown, cut) = bounded_output(&text);
                row = row.child(
                    v_flex()
                        .gap_1()
                        .p_2()
                        .rounded_md()
                        .bg(cx.theme().muted)
                        .debug_selector(|| "automation-run-output".into())
                        .children(cut.then(|| {
                            div()
                                .text_color(cx.theme().muted_foreground)
                                .child("Earlier output is not shown.")
                        }))
                        .child(div().whitespace_normal().child(if shown.is_empty() {
                            SharedString::from(if run.status.is_active() {
                                "Waiting for output…"
                            } else {
                                "No output."
                            })
                        } else {
                            SharedString::from(shown)
                        })),
                );
            }
            list = list.child(row);
        }
        list.children(self.pager("automation-history", runs.len(), page, cx))
            .into_any_element()
    }
}

impl Render for AutomationView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = if let Some(form) = &self.form {
            self.render_form(form, cx)
        } else if !self.snapshot.loaded {
            div()
                .whitespace_normal()
                .child(if self.handle.is_none() {
                    "Automation has not started."
                } else if self.snapshot.connection_error.is_some() {
                    "Jobs appear when the automation backend is reachable. Buddy keeps trying."
                } else {
                    "Loading jobs, schedules and runs…"
                })
                .into_any_element()
        } else {
            match self.pane {
                Pane::Jobs => self.render_jobs(cx),
                Pane::Schedules => self.render_schedules(cx),
                Pane::History => self.render_history(cx),
            }
        };
        let filter = self.form.is_none().then(|| {
            Self::labeled(
                "Filter",
                div().max_w(zpx(420.)).child(Input::new(&self.filter)),
            )
        });
        v_flex()
            .id("automation-view")
            .size_full()
            .overflow_y_scroll()
            .p_4()
            .gap_3()
            .child(self.render_header(cx))
            .child(self.render_status(cx))
            .children(self.render_confirmations(cx))
            .children(filter)
            .child(content)
    }
}
