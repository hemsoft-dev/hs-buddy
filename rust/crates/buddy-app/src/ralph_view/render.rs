//! Dashboard, launch form and run detail.
use buddy_core::ralph::launch::Script;
use buddy_core::ralph::progress::{Phase, Stats};
use buddy_core::ralph::store::{Run, RunState};
use gpui_kit::base::{Disableable, Selectable};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, Textarea};
use gpui_kit::component::select::Select;
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::{
    AnyElement, App, Context, Hsla, InteractiveElement, IntoElement, ParentElement,
    PathPromptOptions, SharedString, StatefulInteractiveElement, Styled, Window, div,
};

use super::form::{Form, script_name};
use super::{Page, RalphView};
use crate::zoom::zpx;

/// Log lines shown in the detail view; the file keeps everything.
const SHOWN_LINES: usize = 400;

pub(super) const BUILT_IN: [(&str, &str); 3] = [
    (
        "Ralph Loop",
        "Full loop: work iterations, then a pull request taken through review.",
    ),
    (
        "Ralph PR",
        "Take an existing pull request through CI and review comments.",
    ),
    (
        "Ralph Issues",
        "Scan the repository and file GitHub issues for what it finds.",
    ),
];

pub(super) fn phase_label(phase: Phase) -> &'static str {
    match phase {
        Phase::Initializing => "Initializing",
        Phase::Iterating => "Iterating",
        Phase::Scanning => "Scanning",
        Phase::PrHandoff => "Opening the pull request",
        Phase::PrResolving => "Resolving review",
    }
}

/// `45s`, `3m 05s` or `2h 07m`.
pub(super) fn duration(milliseconds: i64) -> String {
    let seconds = (milliseconds / 1000).max(0);
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3600 => format!("{}m {:02}s", seconds / 60, seconds % 60),
        _ => format!("{}h {:02}m", seconds / 3600, seconds % 3600 / 60),
    }
}

pub(super) fn local_time(milliseconds: i64) -> String {
    chrono::DateTime::from_timestamp_millis(milliseconds)
        .map(|time| {
            time.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|| "Unknown time".into())
}

/// Counters worth showing; zeros are left out.
pub(super) fn stats(stats: &Stats) -> Vec<String> {
    let mut parts = vec![];
    for (count, label) in [
        (stats.scan_iterations, "scans"),
        (stats.issues_created, "issues created"),
        (stats.checks, "checks"),
        (stats.agent_turns, "agent reviews"),
        (stats.reviews, "review rounds"),
        (stats.copilot_prs, "Copilot reviews"),
    ] {
        if count > 0 {
            parts.push(format!("{count} {label}"));
        }
    }
    if let Some(cost) = &stats.total_cost {
        parts.push(match stats.total_premium {
            Some(premium) => format!("{cost} ({premium} premium requests)"),
            None => cost.clone(),
        });
    }
    parts
}

/// `repo · Ralph PR #42`.
pub(super) fn title(run: &Run, templates: &[buddy_core::ralph::scripts::Template]) -> String {
    let launch = &run.request.launch;
    let repository = launch
        .repository
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| launch.repository.display().to_string());
    let number = launch
        .pr_number
        .filter(|_| launch.script == Script::Pr)
        .map(|number| format!(" #{number}"))
        .unwrap_or_default();
    format!(
        "{repository} · {}{number}",
        script_name(&launch.script, templates)
    )
}

/// What the run is doing now, for example `Iterating · iteration 2 of 3 · cycle 1 of 2`.
pub(super) fn progress(run: &Run) -> String {
    let launch = &run.request.launch;
    let mut parts = vec![];
    if matches!(run.state, RunState::Running | RunState::Stopping)
        && let Some(status) = &run.status
    {
        parts.push(phase_label(status.progress.phase).to_owned());
        if let Some(iteration) = status.progress.iteration {
            parts.push(if launch.script.uses_iterations() {
                format!("iteration {iteration} of {}", launch.iterations)
            } else {
                format!("iteration {iteration}")
            });
        }
        if run.request.plan.cycles > 1 {
            parts.push(format!(
                "cycle {} of {}",
                status.cycle, run.request.plan.cycles
            ));
        }
    }
    let started = run
        .status
        .as_ref()
        .map_or(run.request.created_at, |status| status.started_at);
    let ended = run
        .status
        .as_ref()
        .and_then(|status| status.finished_at)
        .unwrap_or_else(buddy_core::ralph::store::now_ms);
    if run.state != RunState::Interrupted {
        parts.push(duration(ended - started));
    }
    parts.join(" · ")
}

fn state_color(state: RunState, cx: &App) -> Hsla {
    let theme = cx.theme();
    match state {
        RunState::Starting | RunState::Running => theme.info,
        RunState::Stopping | RunState::Stopped | RunState::Interrupted => theme.warning,
        RunState::Completed => theme.success,
        RunState::Failed => theme.danger,
    }
}

fn labeled(label: impl Into<SharedString>, field: impl IntoElement) -> gpui_kit::Div {
    v_flex()
        .gap_1()
        .child(div().child(label.into()))
        .child(field)
}

fn muted(text: impl Into<SharedString>, cx: &App) -> gpui_kit::Div {
    div()
        .whitespace_normal()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}

impl RalphView {
    fn script_description(&self, script: &Script) -> String {
        match script {
            Script::Ralph => BUILT_IN[0].1.into(),
            Script::Pr => BUILT_IN[1].1.into(),
            Script::Issues => BUILT_IN[2].1.into(),
            Script::Template(file) => self
                .templates
                .iter()
                .find(|template| &template.file == file)
                .map(|template| format!("{}.", template.description))
                .unwrap_or_default(),
        }
    }

    pub(super) fn new_form(
        &mut self,
        script: Script,
        previous: Option<&buddy_core::ralph::launch::Launch>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(catalog) = self.catalog.clone() else {
            self.error = Some(
                self.catalog_error
                    .clone()
                    .unwrap_or_else(|| "The Ralph scripts are still loading.".into()),
            );
            cx.notify();
            return;
        };
        let repository = self.last_repository();
        let templates = self.templates.clone();
        let form = Form::new(
            script, previous, repository, &catalog, &templates, window, cx,
        );
        self.open_launch(form, cx);
    }

    fn render_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut buttons = h_flex().flex_wrap().gap_2();
        match self.page {
            Page::Dashboard => {
                buttons = buttons
                    .child(
                        Button::new("ralph-refresh")
                            .label("Refresh")
                            .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                    )
                    .child(
                        Button::new("ralph-new")
                            .label("New loop")
                            .disabled(self.catalog.is_none() || self.busy)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.new_form(Script::Ralph, None, window, cx)
                            })),
                    );
            }
            Page::Launch | Page::Detail(_) => {
                buttons = buttons.child(
                    Button::new("ralph-back")
                        .label("All loops")
                        .disabled(self.busy)
                        .on_click(cx.listener(|this, _, _, cx| this.open_dashboard(cx))),
                );
            }
        }
        h_flex()
            .flex_wrap()
            .items_center()
            .gap_3()
            .debug_selector(|| "ralph-header".into())
            .child(div().text_xl().child("Ralph Loops"))
            .child(buttons)
            .into_any_element()
    }

    fn render_messages(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut messages = v_flex().gap_2();
        if let Some(notice) = &self.notice {
            messages = messages.child(div().whitespace_normal().child(notice.clone()));
        }
        if let Some(error) = &self.error {
            messages = messages.child(
                div()
                    .whitespace_normal()
                    .text_color(cx.theme().danger)
                    .debug_selector(|| "ralph-error".into())
                    .child(error.clone()),
            );
        }
        messages.into_any_element()
    }

    fn render_scripts(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut scripts: Vec<(Script, String, String)> = vec![
            (Script::Ralph, BUILT_IN[0].0.into(), BUILT_IN[0].1.into()),
            (Script::Pr, BUILT_IN[1].0.into(), BUILT_IN[1].1.into()),
            (Script::Issues, BUILT_IN[2].0.into(), BUILT_IN[2].1.into()),
        ];
        scripts.extend(self.templates.iter().map(|template| {
            (
                Script::Template(template.file.clone()),
                template.name.clone(),
                format!("{}.", template.description),
            )
        }));
        let mut list = v_flex()
            .gap_2()
            .debug_selector(|| "ralph-scripts".into())
            .child(div().text_lg().child("Start a loop"));
        if self.catalog.is_none() {
            list = list.child(muted("Loading scripts…", cx));
        }
        for (index, (script, name, description)) in scripts.into_iter().enumerate() {
            list = list.child(
                h_flex()
                    .flex_wrap()
                    .items_center()
                    .gap_3()
                    .child(
                        div().w(zpx(240.)).child(
                            Button::new(("ralph-script", index))
                                .label(name)
                                .w_full()
                                .disabled(self.catalog.is_none() || self.busy)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.new_form(script.clone(), None, window, cx)
                                })),
                        ),
                    )
                    .child(muted(description, cx).flex_1().min_w(zpx(200.))),
            );
        }
        list.into_any_element()
    }

    fn render_run_row(&self, index: usize, run: &Run, cx: &mut Context<Self>) -> AnyElement {
        let id = run.request.id.clone();
        let (open, act) = (id.clone(), id.clone());
        let mut buttons = h_flex().flex_wrap().gap_2().child(
            Button::new(("ralph-open", index))
                .label("Open")
                .on_click(cx.listener(move |this, _, _, cx| this.open_detail(open.clone(), cx))),
        );
        if run.state.is_active() {
            buttons = buttons.child(
                Button::new(("ralph-row-stop", index))
                    .label(if run.state == RunState::Stopping {
                        "Stopping…"
                    } else {
                        "Stop"
                    })
                    .disabled(self.busy || run.state == RunState::Stopping)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        // Stop is confirmed on the run's own page, next to its log.
                        this.open_detail(act.clone(), cx);
                        this.stop_confirm = Some(act.clone());
                    })),
            );
        } else {
            buttons = buttons.child(
                Button::new(("ralph-row-retry", index))
                    .label("Retry")
                    .disabled(self.busy)
                    .on_click(cx.listener(move |this, _, _, cx| this.retry(act.clone(), cx))),
            );
        }
        let launch = &run.request.launch;
        let mut row = v_flex()
            .gap_1()
            .pb_3()
            .border_b_1()
            .border_color(cx.theme().border)
            .debug_selector(move || format!("ralph-run-{index}"))
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        div()
                            .text_color(state_color(run.state, cx))
                            .child(run.state.label()),
                    )
                    .child(div().child(title(run, &self.templates))),
            )
            .child(muted(
                format!(
                    "{} · {} · {}",
                    if launch.model.is_empty() {
                        "default model"
                    } else {
                        &launch.model
                    },
                    branch(run),
                    progress(run)
                ),
                cx,
            ));
        if let Some(error) = run.error() {
            row = row.child(
                div()
                    .whitespace_normal()
                    .text_color(cx.theme().danger)
                    .child(error),
            );
        }
        row.child(buttons).into_any_element()
    }

    fn render_dashboard(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut body = v_flex().gap_4().child(self.render_scripts(cx));
        if !self.loaded {
            return body.child(muted("Loading loops…", cx)).into_any_element();
        }
        let (active, recent): (Vec<&Run>, Vec<&Run>) =
            self.runs.iter().partition(|run| run.state.is_active());
        if self.runs.is_empty() {
            body = body.child(muted(
                "No runs yet. Choose a script above to start a loop.",
                cx,
            ));
        }
        for (heading, runs, offset) in [("Active", active, 0), ("Recent", recent, 1000)] {
            if runs.is_empty() {
                continue;
            }
            let mut section = v_flex()
                .gap_3()
                .debug_selector(move || format!("ralph-{}", heading.to_lowercase()))
                .child(div().text_lg().child(format!("{heading} ({})", runs.len())));
            // Every retained run (the store keeps the newest 100 finished ones), so each
            // can be opened, retried or stopped.
            for (index, run) in runs.into_iter().enumerate() {
                section = section.child(self.render_run_row(offset + index, run, cx));
            }
            body = body.child(section);
        }
        body.into_any_element()
    }

    /// An on/off option, shown and announced as a checkbox.
    fn option(
        id: &'static str,
        label: &'static str,
        on: bool,
        disabled: bool,
        cx: &mut Context<Self>,
        set: impl Fn(&mut Form, bool) + 'static,
    ) -> Checkbox {
        let view = cx.entity().downgrade();
        Checkbox::new(id)
            .label(label)
            .checked(on)
            .disabled(disabled)
            .on_click(move |checked, _, cx| {
                let checked = *checked;
                let _ = view.update(cx, |this, cx| {
                    if let Some(form) = &mut this.form {
                        set(form, checked);
                    }
                    cx.notify();
                });
            })
    }

    fn render_form(&self, form: &Form, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.busy;
        let script = form.script.clone();
        let input = |name: &str| Input::new(&form.fields[name]).disabled(busy);
        let mut body = v_flex()
            .gap_3()
            .p_3()
            .border_1()
            .rounded_md()
            .border_color(cx.theme().border)
            .debug_selector(|| "ralph-form".into())
            // Launch stays reachable above the long fields.
            .child(
                h_flex()
                    .flex_wrap()
                    .items_center()
                    .gap_3()
                    .debug_selector(|| "ralph-form-actions".into())
                    .child(div().text_lg().child(script_name(&script, &self.templates)))
                    .child(
                        Button::new("ralph-launch")
                            // Plain like Save elsewhere: Tesseract cannot read white on the
                            // light theme's primary blue, so installed qualification could not click it.
                            .label(if busy { "Launching…" } else { "Launch" })
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| this.launch(cx))),
                    )
                    .child(
                        Button::new("ralph-close")
                            .label("Close")
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| this.open_dashboard(cx))),
                    ),
            )
            .child(muted(self.script_description(&script), cx))
            .child(labeled(
                "Repository",
                h_flex()
                    .gap_2()
                    .child(div().flex_1().min_w(zpx(240.)).child(input("repository")))
                    .child(
                        Button::new("ralph-browse")
                            .label("Browse…")
                            .disabled(busy)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.browse_folder(window, cx)),
                            ),
                    ),
            ))
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_3()
                    // One row on the 1024 px Windows runner: 240 + 360 + 240 plus gaps.
                    .child(
                        div()
                            .debug_selector(|| "ralph-provider".into())
                            .w(zpx(240.))
                            .child(labeled("Provider", Select::new(&form.provider))),
                    )
                    .child(
                        // Wide enough for "Default (medium → claude-opus-4.6)" with Ubuntu's font.
                        div()
                            .w(zpx(360.))
                            .child(labeled("Model", Select::new(&form.model))),
                    )
                    .child(
                        div()
                            .debug_selector(|| "ralph-agent".into())
                            .w(zpx(240.))
                            .child(labeled("Work agent", Select::new(&form.agent))),
                    ),
            );
        let mut reviews = h_flex().flex_wrap().gap_2();
        for (index, (role, description)) in form.reviews.iter().enumerate() {
            let toggled = role.clone();
            reviews = reviews.child(
                Button::new(("ralph-review", index))
                    .label(role.clone())
                    .tooltip(description.clone())
                    .selected(form.selected.contains_key(role))
                    .toggled(form.selected.contains_key(role))
                    .disabled(busy)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Some(mut form) = this.form.take() {
                            form.toggle_review(&toggled, None, window, cx);
                            this.form = Some(form);
                        }
                        cx.notify();
                    })),
            );
        }
        // ralph-issues.ps1 opens no pull request, so it runs no review agents.
        let reviewed = script != Script::Issues;
        if reviewed {
            body = body.child(labeled("Review agents", reviews));
        }
        if reviewed && !form.selected.is_empty() {
            let mut models = h_flex().flex_wrap().gap_3();
            for (role, picker) in &form.selected {
                models = models.child(
                    div()
                        .w(zpx(280.))
                        .child(labeled(format!("{role} model"), Select::new(picker))),
                );
            }
            body = body.child(models);
        }
        let mut numbers = h_flex().flex_wrap().gap_3();
        if script.uses_iterations() {
            numbers = numbers.child(div().w(zpx(160.)).child(labeled(
                if script == Script::Issues {
                    "Scan iterations"
                } else {
                    "Work iterations"
                },
                input("iterations"),
            )));
        }
        if script.uses_cycles() {
            numbers = numbers.child(
                div()
                    .w(zpx(160.))
                    .child(labeled("Repeat cycles", input("cycles"))),
            );
        }
        if script == Script::Pr {
            numbers = numbers.child(
                div()
                    .w(zpx(160.))
                    .child(labeled("Pull request number", input("pr"))),
            );
        }
        if matches!(script, Script::Ralph | Script::Template(_)) {
            numbers = numbers.child(
                div()
                    .w(zpx(320.))
                    .child(labeled("Branch prefix (optional)", input("branch"))),
            );
        }
        if script == Script::Issues {
            numbers = numbers.child(
                div()
                    .w(zpx(320.))
                    .child(labeled("Issue labels", input("labels"))),
            );
        }
        body = body.child(numbers);
        if script != Script::Pr {
            body = body.child(labeled(
                "Prompt",
                Textarea::new(&form.prompt).disabled(busy),
            ));
        }
        let mut options = h_flex().flex_wrap().gap_2();
        if script == Script::Issues {
            options = options.child(Self::option(
                "ralph-dry-run",
                "Dry run: scan without creating issues",
                form.dry_run,
                busy,
                cx,
                |form, on| form.dry_run = on,
            ));
        } else if script.can_merge() {
            options = options.child(Self::option(
                "ralph-merge",
                "Merge the PR when it is clean (gh pr merge --admin)",
                form.merge,
                busy,
                cx,
                |form, on| form.merge = on,
            ));
        }
        body = body.child(options);
        if let Some(account) = &form.account {
            body = body.child(muted(
                format!("Runs on GitHub as {account}, the account the GitHub view showed."),
                cx,
            ));
        }
        body = body.child(muted(
            "The loop runs in its own process and keeps running if Buddy closes; Stop ends only that loop's processes.",
            cx,
        ));
        body.into_any_element()
    }

    pub(super) fn browse_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose the repository".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = paths.await;
            let _ = this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(Ok(Some(paths))) => {
                        if let (Some(path), Some(form)) = (paths.into_iter().next(), &this.form) {
                            form.fields["repository"].update(cx, |input, cx| {
                                input.set_value(path.display().to_string(), window, cx)
                            });
                        }
                    }
                    Ok(Ok(None)) => {}
                    _ => {
                        this.error = Some(
                            "The folder chooser is unavailable. Enter the repository path instead."
                                .into(),
                        )
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn render_detail(&self, id: &str, cx: &mut Context<Self>) -> AnyElement {
        let Some(run) = self.run(id) else {
            return muted(
                if self.loaded {
                    "This run no longer exists."
                } else {
                    "Loading run…"
                },
                cx,
            )
            .into_any_element();
        };
        let launch = &run.request.launch;
        let mut actions = h_flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .debug_selector(|| "ralph-detail-actions".into())
            .child(div().text_lg().child(title(run, &self.templates)))
            .child(
                div()
                    .text_color(state_color(run.state, cx))
                    .child(run.state.label()),
            );
        let (stop, retry, edit) = (id.to_owned(), id.to_owned(), launch.clone());
        if run.state.is_active() {
            if self.stop_confirm.as_deref() == Some(id) {
                let confirm = id.to_owned();
                actions = actions
                    .child(
                        Button::new("ralph-stop-confirm")
                            .label("Stop now")
                            .danger()
                            .disabled(self.busy)
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.stop(confirm.clone(), cx)),
                            ),
                    )
                    .child(
                        Button::new("ralph-stop-keep")
                            .label("Keep running")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.stop_confirm = None;
                                cx.notify();
                            })),
                    );
            } else {
                actions = actions.child(
                    Button::new("ralph-stop")
                        .label(if run.state == RunState::Stopping {
                            "Stopping…"
                        } else {
                            "Stop loop"
                        })
                        .disabled(self.busy || run.state == RunState::Stopping)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.stop_confirm = Some(stop.clone());
                            cx.notify();
                        })),
                );
            }
        } else {
            actions = actions
                .child(
                    Button::new("ralph-retry")
                        .label("Retry")
                        .disabled(self.busy)
                        .on_click(cx.listener(move |this, _, _, cx| this.retry(retry.clone(), cx))),
                )
                .child(
                    Button::new("ralph-edit")
                        .label("Edit and launch")
                        .disabled(self.busy || self.catalog.is_none())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.new_form(edit.script.clone(), Some(&edit), window, cx)
                        })),
                );
        }
        let mut body = v_flex().gap_2().child(actions);
        if self.stop_confirm.as_deref() == Some(id) && run.state.is_active() {
            body = body.child(div().whitespace_normal().child(
                "Stop this loop? Its processes end now. Commits and branches it already made stay.",
            ));
        }
        let started = run
            .status
            .as_ref()
            .map_or(run.request.created_at, |status| status.started_at);
        let place = match &launch.account {
            Some(account) => format!("{} · GitHub: {account}", launch.repository.display()),
            None => launch.repository.display().to_string(),
        };
        body = body
            .child(muted(place, cx))
            .child(muted(
                format!(
                    "Model: {} · Provider: {} · Agent: {} · Branch: {}",
                    or_default(&launch.model),
                    or_default(&launch.provider),
                    or_default(&launch.dev_agent),
                    branch(run)
                ),
                cx,
            ))
            .child(muted(
                format!("Started {} · {}", local_time(started), progress(run)),
                cx,
            ));
        if let Some(status) = &run.status {
            let counters = stats(&status.progress.stats);
            if !counters.is_empty() {
                body = body.child(div().whitespace_normal().child(counters.join(" · ")));
            }
            if let Some(code) = status.exit_code {
                body = body.child(muted(format!("Exit code {code}"), cx));
            }
        }
        if let Some(error) = run.error() {
            body = body.child(
                div()
                    .whitespace_normal()
                    .text_color(cx.theme().danger)
                    .debug_selector(|| "ralph-run-error".into())
                    .child(error),
            );
        }
        let (lines, cut) = &self.output;
        let shown = &lines[lines.len().saturating_sub(SHOWN_LINES)..];
        let earlier = *cut || shown.len() < lines.len();
        let text = if shown.is_empty() {
            if run.state.is_active() {
                "Waiting for output…".to_string()
            } else {
                "No output.".to_string()
            }
        } else {
            shown.join("\n")
        };
        body = body
            .child(div().text_lg().pt_2().child("Log"))
            .children(earlier.then(|| muted("Earlier lines are in the log file.", cx)))
            .child(
                div()
                    .id("ralph-log")
                    .debug_selector(|| "ralph-log".into())
                    .track_scroll(&self.log_scroll)
                    .overflow_y_scroll()
                    .max_h(zpx(360.))
                    .p_2()
                    .rounded_md()
                    .bg(cx.theme().muted)
                    .font_family(cx.theme().mono_font_family.clone())
                    .whitespace_normal()
                    .child(SharedString::from(text)),
            )
            .child(muted(
                format!(
                    "Log file: {}",
                    self.runs_root
                        .as_ref()
                        .map(|root| root.join(id).join("output.log").display().to_string())
                        .unwrap_or_default()
                ),
                cx,
            ));
        body.into_any_element()
    }

    pub(super) fn render_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.catalog.is_some()
            && let Some(handoff) = self.pending_handoff.take()
        {
            self.notice = None;
            self.open_handoff(handoff, window, cx);
        }
        let content = match self.page.clone() {
            Page::Dashboard => self.render_dashboard(cx),
            Page::Launch => match &self.form {
                Some(form) => self.render_form(form, cx),
                None => self.render_dashboard(cx),
            },
            Page::Detail(id) => self.render_detail(&id, cx),
        };
        v_flex()
            .id("ralph-view")
            .size_full()
            .overflow_y_scroll()
            .p_4()
            .gap_3()
            .child(self.render_header(cx))
            .child(self.render_messages(cx))
            .child(content)
            .into_any_element()
    }
}

/// The branch the loop printed; until then, what was asked for. ralph.ps1 adds a random
/// suffix to a requested name.
pub(super) fn branch(run: &Run) -> String {
    if let Some(branch) = run
        .status
        .as_ref()
        .and_then(|status| status.progress.branch.clone())
    {
        return branch;
    }
    // Ralph Issues scans the checkout as it is; Ralph PR works on the pull request's branch.
    match run.request.launch.script {
        Script::Issues => return "current checkout".into(),
        Script::Pr => return "the pull request's branch".into(),
        _ => {}
    }
    match run.request.launch.branch.as_str() {
        "" => "created by the loop".into(),
        prefix => format!("{prefix}-… (suffix added by the loop)"),
    }
}

fn or_default(value: &str) -> &str {
    if value.is_empty() { "default" } else { value }
}
