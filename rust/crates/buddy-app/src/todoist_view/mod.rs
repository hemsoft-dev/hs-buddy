//! Native Todoist planning. Service mutations require explicit user actions.
mod editor;
mod tasks;
#[cfg(test)]
mod tests;
use std::sync::Arc;

use buddy_core::todoist::{
    Client, Id, WritePhase, credentials,
    planner::{Coverage, Dataset, Planner, ProjectCatalog, TaskChange},
};
use gpui_kit::base::{Disableable, Selectable};
use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::{
    AppContext as _, Context, Entity, InteractiveElement, IntoElement, ParentElement, Render,
    StatefulInteractiveElement, Styled, Window, div,
};

use crate::runtime::Runtime;
use crate::zoom::zpx;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pane {
    Today,
    Upcoming,
    Projects,
}

enum Operation {
    Refresh,
    Load(Coverage, Pane, Option<Id>),
    Submit(TaskChange),
    Retry,
    Cleanup,
    Discard,
    Recovery,
}
type ConnectionResult = Result<Option<(Planner, Option<String>)>, String>;
type OperationResult = (Planner, Result<String, String>);

pub struct TodoistView {
    planner: Option<Planner>,
    data: Option<Arc<Dataset>>,
    catalog: Option<Arc<ProjectCatalog>>,
    token: Entity<InputState>,
    filter: Entity<InputState>,
    pane: Pane,
    project: Option<Id>,
    task_page: usize,
    project_page: usize,
    editor: Option<editor::Editor>,
    delete_target: Option<Id>,
    clear_token: bool,
    initialized: bool,
    busy: bool,
    generation: uuid::Uuid,
    connection_form: bool,
    discard_confirm: bool,
    notice: Option<String>,
    error: Option<String>,
}

impl TodoistView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let token = cx.new(|cx| InputState::new(window, cx).masked(true));
        let filter = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Task name, description, label or project")
        });
        for input in [&token, &filter] {
            cx.subscribe(input, |this, _, _: &InputEvent, cx| {
                this.task_page = 0;
                cx.notify();
            })
            .detach();
        }
        Self {
            planner: None,
            data: None,
            catalog: None,
            token,
            filter,
            pane: Pane::Today,
            project: None,
            task_page: 0,
            project_page: 0,
            editor: None,
            delete_target: None,
            clear_token: false,
            initialized: false,
            busy: false,
            generation: uuid::Uuid::new_v4(),
            connection_form: true,
            discard_confirm: false,
            notice: None,
            error: None,
        }
    }

    pub fn activate(&mut self, cx: &mut Context<Self>) {
        if !self.initialized && cx.has_global::<Runtime>() {
            self.connect(None, cx);
        }
    }

    fn forget_token(&mut self, cx: &mut Context<Self>) {
        if self.busy
            || !cx.has_global::<Runtime>()
            || self
                .planner
                .as_ref()
                .is_some_and(|planner| planner.pending().is_some() || planner.recovery_blocked())
        {
            return;
        }
        self.busy = true;
        self.error = None;
        self.notice = Some("Removing the saved Todoist token…".into());
        if let Some(planner) = &mut self.planner {
            planner.require_refresh();
        }
        let generation = self.generation;
        let rx = Runtime::global(cx).spawn(async {
            let config = buddy_core::config::config_path()
                .map_err(|_| "The configuration profile is unavailable.".to_owned())?;
            credentials::clear_if_no_pending_at(&config).map_err(|error| error.to_string())
        });
        cx.spawn(async move |this, cx| {
            let result = rx.await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.busy = false;
                this.notice = None;
                match result {
                    Ok(Ok(())) => {
                        this.data = None;
                        this.catalog = None;
                        this.planner = None;
                        this.connection_form = true;
                        this.clear_token = true;
                        this.notice = Some(
                            "The saved Todoist token was removed. Connect again to load tasks."
                                .into(),
                        )
                    }
                    Ok(Err(error)) => this.error = Some(error),
                    Err(_) => this.error = Some(
                        "Token removal is unconfirmed. Check secure storage before reconnecting."
                            .into(),
                    ),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn connect(&mut self, entered: Option<String>, cx: &mut Context<Self>) {
        if self.busy || !cx.has_global::<Runtime>() {
            return;
        }
        if self
            .planner
            .as_ref()
            .and_then(Planner::pending)
            .is_some_and(|pending| {
                matches!(
                    pending.phase(),
                    WritePhase::Confirmed | WritePhase::Rejected
                )
            })
        {
            self.error = Some("Finish recovery cleanup before changing this connection.".into());
            cx.notify();
            return;
        }
        let expected = self.planner.as_ref().and_then(|planner| {
            planner.pending().map(|change| {
                (
                    planner.account().clone(),
                    change.request().command_id(),
                    change.request().commands().to_owned(),
                )
            })
        });
        let previous = self.planner.as_mut().map(|planner| {
            planner.require_refresh();
            (planner.account().clone(), planner.coverage().clone())
        });
        let retain_data = self.data.is_some();
        self.busy = true;
        self.initialized = true;
        self.error = None;
        self.notice = Some("Checking Todoist connection and saved recovery…".into());
        self.generation = uuid::Uuid::new_v4();
        let generation = self.generation;
        let rx = Runtime::global(cx).spawn(async move {
            let config = buddy_core::config::config_path().map_err(|_| "The configuration profile is unavailable.".to_owned())?;
            let save = entered.is_some();
            let token = match entered {
                Some(token) => token,
                None => match credentials::load_at(&config).map_err(|error| error.to_string())? {
                    Some(token) => token,
                    None => {
                        if expected.is_some() {
                            return Err("A saved change needs its original Todoist account. Enter a replacement token for that account before retrying.".to_owned());
                        }
                        if retain_data {
                            return Err("No token is saved. Loaded tasks were retained. Enter a token to check this connection again.".to_owned());
                        }
                        return Ok(None);
                    }
                },
            };
            let (client, now) = if token == "native-fixture-todoist-token" {
                let (client, now) = Client::for_desktop_fixture(&token, &config).map_err(|error| error.to_string())?;
                (client, Some(now))
            } else {
                (Client::new(&token).map_err(|error| error.to_string())?, None)
            };
            let mut planner = Planner::connect(client, config.clone(), now).await.map_err(|error| error.to_string())?;
            if let Some((account, uuid, commands)) = expected
                && (planner.account() != &account || planner.pending().is_none_or(|change| {
                    change.request().command_id() != uuid || change.request().commands() != commands
                })) {
                    return Err("The saved change belongs to the original Todoist connection. Its recovery was preserved.".to_owned());
            }
            let coverage = previous.filter(|(account, _)| account == planner.account())
                .map(|(_, coverage)| coverage).unwrap_or(Coverage::All);
            let problem = match planner.refresh_scope(coverage).await {
                Ok(_) => None,
                Err(error) if !retain_data && planner.catalog().is_some()
                    && error == buddy_core::todoist::TransportError::Domain(buddy_core::todoist::Error::ReadLimit) => Some(error.to_string()),
                Err(error) => return Err(error.to_string()),
            };
            if save {
                credentials::save_at(&config, &token).map_err(|error| error.to_string())?;
            }
            Ok::<_, String>(Some((planner, problem)))
        });
        self.receive_connection(generation, rx, cx);
        cx.notify();
    }

    fn receive_connection(
        &self,
        generation: uuid::Uuid,
        rx: futures::channel::oneshot::Receiver<ConnectionResult>,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            let result = rx.await;
            this.update(cx, |this, cx| {
                if this.generation != generation { return; }
                this.busy = false;
                this.notice = None;
                match result {
                    Ok(Ok(Some((planner, problem)))) => {
                        this.data = planner.data().cloned();
                        this.catalog = planner.catalog().cloned();
                        this.planner = Some(planner);
                        this.connection_form = false;
                        this.clear_token = true;
                        this.project = None;
                        this.task_page = 0;
                        this.project_page = 0;
                        this.error = problem;
                    }
                    Ok(Ok(None)) => {
                        this.planner = None;
                        this.connection_form = true;
                        this.notice = Some("Connect Todoist to load your tasks. No API token is saved for this profile.".into());
                    }
                    Ok(Err(error)) => { this.connection_form = this.data.is_none(); this.error = Some(error); }
                    Err(_) => {
                        this.connection_form = this.data.is_none();
                        this.error = Some("The connection check stopped. Recheck secure storage before continuing.".into());
                    }
                }
                cx.notify();
            }).ok();
        }).detach();
    }

    fn operate(&mut self, operation: Operation, cx: &mut Context<Self>) {
        if self.busy || !cx.has_global::<Runtime>() {
            return;
        }
        let Some(mut planner) = self.planner.take() else {
            return;
        };
        self.busy = true;
        self.error = None;
        self.notice = Some("Working with Todoist…".into());
        self.discard_confirm = false;
        let generation = self.generation;
        let submitted = matches!(&operation, Operation::Submit(_));
        let navigation = match &operation {
            Operation::Load(_, pane, project) => Some((*pane, project.clone())),
            _ => None,
        };
        let rx = Runtime::global(cx).spawn(async move {
            let result = match operation {
                Operation::Refresh => planner.refresh().await.map(|_| "Tasks refreshed.".to_owned()).map_err(|error| error.to_string()),
                Operation::Load(coverage, _, _) => planner.refresh_scope(coverage).await.map(|_| "Task scope loaded.".to_owned()).map_err(|error| error.to_string()),
                Operation::Cleanup => planner.retry_cleanup().map(|_| "Recovery cleanup finished. No task change was sent.".to_owned()).map_err(|error| error.to_string()),
                Operation::Discard => planner.discard_pending().map(|_| "Saved recovery discarded. Refresh to check Todoist; the remote change may already exist.".to_owned()).map_err(|error| error.to_string()),
                Operation::Recovery => planner.check_recovery().await.map(|_| "Recovery checked. Saved changes require an explicit retry.".to_owned()).map_err(|error| error.to_string()),
                Operation::Submit(change) => write_result(&mut planner, Some(change)).await,
                Operation::Retry => write_result(&mut planner, None).await,
            };
            (planner, result)
        });
        self.receive_operation(generation, submitted, navigation, rx, cx);
        cx.notify();
    }

    fn receive_operation(
        &self,
        generation: uuid::Uuid,
        submitted: bool,
        navigation: Option<(Pane, Option<Id>)>,
        rx: futures::channel::oneshot::Receiver<OperationResult>,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            let result = rx.await;
            this.update(cx, |this, cx| {
                if this.generation != generation { return; }
                this.busy = false;
                this.notice = None;
                match result {
                    Ok((planner, result)) => {
                        if result.is_ok() && let Some((pane, project)) = navigation {
                            this.pane = pane;
                            this.project = project;
                            this.task_page = 0;
                            this.delete_target = None;
                        }
                        if submitted && (result.is_ok() || planner.pending().is_some()) {
                            this.editor = None;
                        }
                        this.data = planner.data().cloned();
                        this.catalog = planner.catalog().cloned();
                        this.planner = Some(planner);
                        match result { Ok(notice) => this.notice = Some(notice), Err(error) => this.error = Some(error) }
                    }
                    Err(_) => {
                        this.connection_form = this.data.is_none();
                        this.error = Some("The operation stopped. Its outcome may be unconfirmed. Reconnect to check saved recovery before sending another change.".into());
                    }
                }
                cx.notify();
            }).ok();
        }).detach();
    }
}

async fn write_result(planner: &mut Planner, change: Option<TaskChange>) -> Result<String, String> {
    let report = match change {
        Some(change) => planner.submit(change).await,
        None => planner.retry().await,
    }
    .map_err(|error| error.to_string())?;
    let message = match report.phase {
        WritePhase::Confirmed => "Todoist confirmed the change.",
        WritePhase::Rejected => {
            "Todoist rejected the change. Check the task and prepare a new change after refreshing."
        }
        _ => {
            return Err(format!(
                "The change is unconfirmed. It may already exist in Todoist. Retry the saved change to reuse its original command. {}",
                report
                    .transport_problem
                    .map(|error| error.to_string())
                    .unwrap_or_default()
            ));
        }
    };
    if let Some(problem) = report.cleanup_problem {
        if !report.acknowledgement_durable {
            return Ok(format!(
                "{message} The known outcome could not be saved locally. Keep this app open and retry cleanup after restoring file access. No further service write is needed. {problem}"
            ));
        }
        return Ok(format!(
            "{message} {problem} Retry cleanup only; do not send the change again."
        ));
    }
    match planner.refresh().await {
        Ok(_) => Ok(message.into()),
        Err(error) => Ok(format!(
            "{message} The task list could not refresh. {error}"
        )),
    }
}

impl Render for TodoistView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.clear_token {
            self.token
                .update(cx, |state, cx| state.set_value("", window, cx));
            self.clear_token = false;
        }
        let mut tabs = h_flex().flex_wrap().gap_2();
        for (index, (label, pane)) in [
            ("Today", Pane::Today),
            ("Upcoming", Pane::Upcoming),
            ("Projects", Pane::Projects),
        ]
        .into_iter()
        .enumerate()
        {
            tabs = tabs.child(
                Button::new(("todoist-pane", index))
                    .label(label)
                    .selected(self.pane == pane)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select_pane(pane, cx);
                    })),
            );
        }
        let mut body = v_flex()
            .gap_3()
            .child(div().text_xl().child("Task planner"))
            .child(tabs);
        body = body.child(
            h_flex()
                .flex_wrap()
                .gap_2()
                .child(
                    Button::new("todoist-refresh")
                        .label("Refresh")
                        .disabled(self.busy || self.planner.is_none())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.delete_target = None;
                            this.operate(Operation::Refresh, cx);
                        })),
                )
                .child(
                    Button::new("todoist-new-task")
                        .label("New task")
                        .disabled(!self.can_create() || self.editor.is_some())
                        .on_click(
                            cx.listener(|this, _, window, cx| this.open_editor(None, window, cx)),
                        ),
                )
                .child(
                    Button::new("todoist-connection")
                        .label("Connection")
                        .disabled(self.busy || self.editor.is_some())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.connection_form = !this.connection_form;
                            cx.notify();
                        })),
                ),
        );
        if let Some(data) = &self.data {
            body = body.child(
                div()
                    .whitespace_normal()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!(
                        "Account {} · {} · refreshed {} UTC",
                        data.account.as_str(),
                        data.calendar.zone_name(),
                        data.read_at.format("%Y-%m-%d %H:%M")
                    )),
            );
        }
        if let Some(notice) = &self.notice {
            body = body.child(div().whitespace_normal().w_full().child(notice.clone()));
        }
        if let Some(error) = &self.error {
            body = body.child(
                div()
                    .w_full()
                    .whitespace_normal()
                    .text_color(cx.theme().danger)
                    .child(error.clone()),
            );
        }
        if let Some(planner) = &self.planner {
            if let Some(pending) = planner.pending() {
                let acknowledged = matches!(
                    pending.phase(),
                    WritePhase::Confirmed | WritePhase::Rejected
                );
                let recovery_notice = if acknowledged && !pending.acknowledgement_durable() {
                    "The service outcome is known but could not be saved locally. Keep this app open until recovery cleanup succeeds."
                } else if acknowledged {
                    "The service outcome is known. Finish local recovery cleanup before preparing another change."
                } else {
                    "A saved change is unconfirmed. It may already exist in Todoist. An explicit retry sends the same command."
                };
                body = body
                    .child(div().whitespace_normal().child(recovery_notice))
                    .child(
                        h_flex()
                            .flex_wrap()
                            .gap_2()
                            .child(
                                Button::new("todoist-retry-saved")
                                    .label(if acknowledged {
                                        "Retry cleanup"
                                    } else {
                                        "Retry saved change"
                                    })
                                    .disabled(self.busy)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.operate(
                                            if acknowledged {
                                                Operation::Cleanup
                                            } else {
                                                Operation::Retry
                                            },
                                            cx,
                                        )
                                    })),
                            )
                            .child(
                                Button::new("todoist-discard-saved")
                                    .label("Discard saved recovery")
                                    .disabled(self.busy)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.discard_confirm = true;
                                        cx.notify();
                                    })),
                            ),
                    );
                if self.discard_confirm {
                    body=body.child(div().whitespace_normal().child("The change may already exist in Todoist. Discarding removes only local recovery and does not undo the service change."))
                        .child(h_flex().flex_wrap().gap_2()
                            .child(Button::new("todoist-confirm-discard").label("Confirm discard").disabled(self.busy)
                                .on_click(cx.listener(|this, _, _, cx| this.operate(Operation::Discard,cx))))
                            .child(Button::new("todoist-keep-recovery").label("Keep recovery").disabled(self.busy)
                                .on_click(cx.listener(|this, _, _, cx| {this.discard_confirm=false;cx.notify();}))));
                }
            } else if planner.recovery_blocked() {
                body = body.child(
                    Button::new("todoist-check-recovery")
                        .label("Check saved recovery")
                        .disabled(self.busy)
                        .on_click(
                            cx.listener(|this, _, _, cx| this.operate(Operation::Recovery, cx)),
                        ),
                );
            }
        }
        let content = if self.connection_form {
            v_flex()
                .flex_1()
                .min_h_0()
                .gap_3()
                .id("todoist-connection-scroll")
                .overflow_y_scroll()
                .child(div().text_lg().child("Todoist connection"))
                .child(div().child("Todoist API token"))
                .child(Input::new(&self.token).disabled(self.busy))
                .child(
                    div()
                        .whitespace_normal()
                        .text_color(cx.theme().muted_foreground)
                        .child(
                            "Stored in your OS credential store for this configuration profile.",
                        ),
                )
                .child(
                    Button::new("todoist-connect")
                        .label("Save and connect")
                        .disabled(self.busy || self.token.read(cx).value().is_empty())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.connect(Some(this.token.read(cx).value().to_string()), cx)
                        })),
                )
                .child(
                    Button::new("todoist-recheck-connection")
                        .label("Recheck saved connection")
                        .disabled(self.busy)
                        .on_click(cx.listener(|this, _, _, cx| this.connect(None, cx))),
                )
                .child(
                    Button::new("todoist-forget-token")
                        .label("Forget saved token")
                        .disabled(
                            self.busy
                                || self.planner.as_ref().is_some_and(|planner| {
                                    planner.pending().is_some() || planner.recovery_blocked()
                                }),
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.forget_token(cx))),
                )
                .into_any_element()
        } else if self.editor.is_some() {
            self.render_editor(cx)
        } else {
            let filter = Input::new(&self.filter);
            v_flex()
                .flex_1()
                .min_h_0()
                .gap_3()
                .child(div().child("Filter tasks"))
                .child(filter)
                .child(self.render_tasks(cx))
                .into_any_element()
        };
        v_flex()
            .size_full()
            .min_h_0()
            .p_4()
            .gap_3()
            .child(
                body.id("todoist-controls-scroll")
                    .overflow_y_scroll()
                    .max_h(zpx(300.0))
                    .flex_shrink_0(),
            )
            .child(content)
    }
}
