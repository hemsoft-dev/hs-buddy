//! Project list, conversation, composer and changed files.
use buddy_core::crew::changes::{Change, Kind};
use buddy_core::crew::{Message, Project, Role, Session, Status};
use gpui_kit::base::{Disableable, Selectable};
use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Input, Textarea};
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window, div,
};

use super::{Confirm, CrewView};
use crate::copilot_view::markdown;
use crate::zoom::zpx;

fn muted(text: impl Into<SharedString>, cx: &App) -> gpui_kit::Div {
    div()
        .whitespace_normal()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}

/// A field under its label; installed qualification finds fields by their labels.
fn labeled(label: &'static str, field: impl IntoElement) -> gpui_kit::Div {
    v_flex().gap_1().child(div().child(label)).child(field)
}

pub(super) fn kind_label(kind: Kind) -> &'static str {
    match kind {
        Kind::Added => "Added",
        Kind::Modified => "Modified",
        Kind::Deleted => "Deleted",
    }
}

fn message_label(message: &Message) -> &'static str {
    match (message.role, message.error, message.partial) {
        (Role::User, _, _) => "You",
        (Role::Assistant, true, _) => "Copilot, failed",
        (Role::Assistant, false, true) => "Copilot, partial response",
        (Role::Assistant, false, false) => "Copilot",
    }
}

impl CrewView {
    /// What the project list says about a project's conversation.
    pub(super) fn project_state(&self, project: &Project) -> String {
        if self.turns.contains_key(&project.id) || self.sending.contains(&project.id) {
            return "Replying…".into();
        }
        match self.sessions.get(&project.id) {
            None => "No conversation".into(),
            Some(session) if session.status == Status::Error => "Last reply failed".into(),
            Some(session) if session.pending.is_some() => "Replying in another window".into(),
            Some(session) => match session.conversation_history.len() {
                1 => "1 message".into(),
                count => format!("{count} messages"),
            },
        }
    }

    fn render_projects(&self, cx: &mut Context<Self>) -> AnyElement {
        let (primary, border) = (cx.theme().primary, cx.theme().border);
        let mut list = v_flex()
            .gap_2()
            .debug_selector(|| "crew-projects".into())
            .child(div().text_xl().child("The Crew"))
            .child(muted(
                "Add a local clone of a GitHub repository to talk about it with Copilot.",
                cx,
            ))
            .child(labeled(
                "Project folder",
                Input::new(&self.path).disabled(self.busy),
            ))
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        Button::new("crew-add")
                            .label(if self.busy {
                                "Working…"
                            } else {
                                "Add project"
                            })
                            .disabled(self.busy || self.path.read(cx).value().trim().is_empty())
                            .on_click(
                                cx.listener(|this, _, window, cx| this.add_project(window, cx)),
                            ),
                    )
                    .child(
                        Button::new("crew-browse")
                            .label("Browse…")
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, window, cx| this.browse(window, cx))),
                    ),
            );
        if !self.loaded {
            list = list.child(muted("Loading projects…", cx));
        } else if self.projects.is_empty() {
            list = list.child(muted("No projects yet.", cx));
        }
        for (index, project) in self.projects.iter().enumerate() {
            let selected = self.selected.as_ref() == Some(&project.id);
            let id = project.id.clone();
            let remove = project.id.clone();
            let confirming = self.confirm == Some(Confirm::Remove(project.id.clone()));
            let replying = self.replying(&project.id);
            list = list.child(
                v_flex()
                    .id(("crew-project", index))
                    .gap_1()
                    .p_2()
                    .rounded_md()
                    .border_1()
                    .border_color(if selected { primary } else { border })
                    .when_selected(selected, cx)
                    .cursor_pointer()
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.select(id.clone(), window, cx)),
                    )
                    .child(div().child(project.display_name.clone()))
                    .child(muted(project.github_slug.clone(), cx))
                    .child(muted(self.project_state(project), cx))
                    .when_some(
                        confirming.then_some(
                            "Forget this project and its conversation? Its folder is not changed.",
                        ),
                        |card, question| card.child(muted(question, cx)),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new(("crew-remove", index))
                                    .label(if confirming {
                                        "Confirm remove"
                                    } else {
                                        "Remove"
                                    })
                                    .disabled(self.busy || replying)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.remove(remove.clone(), cx)
                                    })),
                            )
                            .when(confirming, |row| {
                                row.child(self.cancel_button(("crew-remove-cancel", index), cx))
                            }),
                    ),
            );
        }
        v_flex()
            .id("crew-project-list")
            .w(zpx(280.))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .p_4()
            .border_r_1()
            .border_color(border)
            .child(list)
            .into_any_element()
    }

    fn render_messages(&self, cx: &App) -> AnyElement {
        let mut messages = v_flex().gap_1();
        if let Some(notice) = &self.notice {
            messages = messages.child(
                div()
                    .whitespace_normal()
                    .debug_selector(|| "crew-notice".into())
                    .child(notice.clone()),
            );
        }
        if let Some(error) = &self.error {
            messages = messages.child(
                div()
                    .whitespace_normal()
                    .text_color(cx.theme().danger)
                    .debug_selector(|| "crew-error".into())
                    .child(error.clone()),
            );
        }
        messages.into_any_element()
    }

    fn render_start(&self, project: &Project, cx: &mut Context<Self>) -> AnyElement {
        let accounts = Self::accounts(cx);
        let mut choices = h_flex().flex_wrap().gap_2();
        for (index, account) in accounts.iter().enumerate() {
            let name = account.username.clone();
            let chosen = self
                .account
                .as_ref()
                .is_some_and(|chosen| chosen.eq_ignore_ascii_case(&name));
            choices = choices.child(
                Button::new(("crew-account", index))
                    .label(name.clone())
                    .selected(chosen)
                    .toggled(chosen)
                    .disabled(self.busy)
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.choose_account(name.clone(), cx)),
                    ),
            );
        }
        let model_empty = self.model.read(cx).value().trim().is_empty();
        v_flex()
            .gap_3()
            .p_3()
            .border_1()
            .rounded_md()
            .border_color(cx.theme().border)
            .debug_selector(|| "crew-start".into())
            .child(div().text_lg().child("Start a conversation"))
            .child(muted(
                format!(
                    "Copilot answers questions about {} as the account you choose. It cannot \
                     change files. From now on, Changed files lists what changes in this folder, \
                     by anyone, so each change can be kept or undone.",
                    project.github_slug
                ),
                cx,
            ))
            .child(div().child("GitHub account"))
            .child(if accounts.is_empty() {
                muted("Add a GitHub account in Settings first.", cx).into_any_element()
            } else {
                choices.into_any_element()
            })
            .child(div().w(zpx(320.)).child(labeled(
                "Model",
                Input::new(&self.model).disabled(self.busy),
            )))
            .child(
                h_flex().child(
                    Button::new("crew-start-session")
                        .label(if self.busy {
                            "Starting…"
                        } else {
                            "Start conversation"
                        })
                        .disabled(self.busy || self.account.is_none() || model_empty)
                        .on_click(cx.listener(|this, _, _, cx| this.start_session(cx))),
                ),
            )
            .into_any_element()
    }

    fn render_conversation(&self, project: &Project, session: &Session, cx: &App) -> AnyElement {
        let mut body = v_flex()
            .id("crew-conversation")
            .track_scroll(&self.conversation_scroll)
            .flex_1()
            .min_h(zpx(110.))
            .gap_3()
            .overflow_y_scroll()
            .debug_selector(|| "crew-conversation".into());
        if session.conversation_history.is_empty() && !self.turns.contains_key(&project.id) {
            body = body.child(muted("Send a message to start the conversation.", cx));
        }
        for (index, message) in session.conversation_history.iter().enumerate() {
            let mut entry = v_flex().gap_1().child(muted(message_label(message), cx));
            entry = match message.role {
                Role::User => entry.child(div().whitespace_normal().child(message.content.clone())),
                Role::Assistant if message.error => entry.child(
                    div()
                        .whitespace_normal()
                        .text_color(cx.theme().danger)
                        .child(message.content.clone()),
                ),
                Role::Assistant => entry.child(markdown::response(
                    ("crew-message", index),
                    message.content.clone(),
                )),
            };
            body = body.child(entry);
        }
        if let Some(active) = self.turns.get(&project.id) {
            let live = active.turn.live();
            let mut entry = v_flex().gap_1().child(muted(
                if live.stopping {
                    "Copilot is stopping…"
                } else {
                    "Copilot is replying…"
                },
                cx,
            ));
            if !live.output.is_empty() {
                entry = entry.child(markdown::response("crew-live-reply", live.output));
            }
            body = body.child(entry);
        } else if self.sending.contains(&project.id) {
            body = body.child(muted("Sending…", cx));
        }
        body.into_any_element()
    }

    fn render_change(&self, index: usize, change: &Change, cx: &mut Context<Self>) -> AnyElement {
        let lines = change
            .lines
            .map(|(added, removed)| format!("+{added} −{removed}"))
            .unwrap_or_default();
        let confirming = self.confirm == Some(Confirm::Undo(change.path.clone()));
        let (keep_path, keep_seen) = (change.path.clone(), change.seen.clone());
        let (undo_path, undo_seen) = (change.path.clone(), change.seen.clone());
        let mut row = h_flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .min_w(zpx(160.))
                    .whitespace_normal()
                    .child(change.path.clone()),
            )
            .child(muted(kind_label(change.kind), cx))
            .child(muted(lines, cx))
            .child(
                Button::new(("crew-keep", index))
                    .label("Keep")
                    .disabled(self.acting)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.keep(keep_path.clone(), keep_seen.clone(), cx)
                    })),
            )
            .child(
                Button::new(("crew-undo", index))
                    .label(if confirming {
                        "Confirm undo"
                    } else {
                        "Undo change"
                    })
                    .disabled(self.acting || change.no_undo.is_some())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.undo(undo_path.clone(), undo_seen.clone(), cx)
                    })),
            );
        if confirming {
            row = row
                .child(self.cancel_button(("crew-undo-cancel", index), cx))
                .child(
                    muted(
                        "Restore the file as it was when this conversation started? \
                         Later edits to it are lost.",
                        cx,
                    )
                    .w_full(),
                );
        }
        if let Some(reason) = change.no_undo {
            row = row.child(muted(reason, cx).w_full());
        }
        row.into_any_element()
    }

    fn render_changes(&self, session: &Session, cx: &mut Context<Self>) -> AnyElement {
        let mut panel = v_flex()
            .id("crew-changes")
            .flex_none()
            // About four rows; the conversation keeps the rest of a compact window.
            .max_h(zpx(150.))
            .overflow_y_scroll()
            .gap_2()
            .pt_2()
            .border_t_1()
            .border_color(cx.theme().border)
            .debug_selector(|| "crew-changes".into());
        let mut header = h_flex()
            .flex_wrap()
            .items_center()
            .gap_3()
            .child(div().text_lg().child("Changed files"))
            .child(
                Button::new("crew-refresh-changes")
                    .label("Refresh")
                    .disabled(self.scanning)
                    .on_click(cx.listener(|this, _, _, cx| this.scan(cx))),
            );
        let current = self
            .changes
            .as_ref()
            .filter(|changes| changes.session == session.id);
        match current.map(|changes| &changes.scan) {
            None => {
                header = header.child(muted("Reading…", cx));
                panel = panel.child(header);
            }
            Some(Err(error)) => {
                panel = panel.child(header).child(
                    div()
                        .whitespace_normal()
                        .text_color(cx.theme().danger)
                        .child(error.clone()),
                );
            }
            Some(Ok(scan)) if scan.changes.is_empty() => {
                panel = panel
                    .child(header)
                    .child(muted("No changes since this conversation started.", cx));
            }
            Some(Ok(scan)) => {
                let count = scan.changes.len();
                header = header.child(muted(
                    if count == 1 {
                        "1 file".to_string()
                    } else {
                        format!("{count} files")
                    },
                    cx,
                ));
                panel = panel.child(header);
                for (index, change) in scan.changes.iter().enumerate() {
                    panel = panel.child(self.render_change(index, change, cx));
                }
                if scan.truncated {
                    panel = panel.child(muted(
                        format!(
                            "Only the first {} changed files are listed.",
                            buddy_core::crew::changes::MAX_LISTED
                        ),
                        cx,
                    ));
                }
            }
        }
        panel.into_any_element()
    }

    fn render_composer(&self, project: &Project, cx: &mut Context<Self>) -> AnyElement {
        let replying = self.replying(&project.id);
        let confirming = self.confirm == Some(Confirm::Clear(project.id.clone()));
        let empty = self.message.read(cx).value().trim().is_empty();
        let stop = project.id.clone();
        let clear = project.id.clone();
        let mut buttons = h_flex().flex_wrap().gap_2().child(
            Button::new("crew-send")
                .label("Send message")
                .disabled(replying || self.busy || empty)
                .on_click(cx.listener(|this, _, window, cx| this.send(window, cx))),
        );
        if self.turns.contains_key(&project.id) {
            buttons = buttons.child(
                Button::new("crew-stop")
                    .label("Stop reply")
                    .on_click(cx.listener(move |this, _, _, cx| this.stop(&stop, cx))),
            );
        }
        buttons = buttons.child(
            Button::new("crew-clear")
                .label(if confirming {
                    "Confirm clear"
                } else {
                    "Clear conversation"
                })
                .disabled(replying || self.busy)
                .on_click(cx.listener(move |this, _, _, cx| this.clear(clear.clone(), cx))),
        );
        if confirming {
            buttons = buttons.child(self.cancel_button("crew-clear-cancel", cx));
        }
        v_flex()
            .flex_none()
            .gap_2()
            .debug_selector(|| "crew-composer".into())
            .child(labeled(
                "Message",
                div()
                    .h(zpx(56.))
                    .child(Textarea::new(&self.message).disabled(replying)),
            ))
            .child(muted("Enter sends · Shift+Enter adds a newline", cx))
            .when(confirming, |composer| {
                composer.child(muted(
                    "Delete this conversation and its snapshot? Project files are not changed.",
                    cx,
                ))
            })
            .child(buttons)
            .into_any_element()
    }

    fn render_detail(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(project) = self.selected_project().cloned() else {
            return v_flex()
                .flex_1()
                .h_full()
                .p_4()
                .gap_3()
                .child(self.render_messages(cx))
                .child(muted(
                    if self.projects.is_empty() {
                        "Add a project folder on the left to start."
                    } else {
                        "Choose a project on the left."
                    },
                    cx,
                ))
                .into_any_element();
        };
        let session = self.selected_session().cloned();
        // The conversation's account and model share the repository's line, so a
        // compact window keeps its height for the conversation.
        let identity = match &session {
            Some(session) => format!(
                "{} · as {} · {}",
                project.github_slug, session.account, session.model
            ),
            None => project.github_slug.clone(),
        };
        let header = v_flex()
            .gap_1()
            .debug_selector(|| "crew-header".into())
            .child(
                h_flex()
                    .flex_wrap()
                    .items_center()
                    .gap_3()
                    .child(div().text_xl().child(project.display_name.clone()))
                    .child(muted(identity, cx)),
            )
            .child(muted(
                format!(
                    "Branch {} · {}",
                    project.default_branch,
                    project.local_path.display()
                ),
                cx,
            ));
        let mut page = v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .p_4()
            .gap_3()
            .child(header)
            .child(self.render_messages(cx));
        match session {
            None => {
                page = page.child(
                    v_flex()
                        .id("crew-start-scroll")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .child(self.render_start(&project, cx)),
                );
            }
            Some(session) => {
                page = page
                    .child(self.render_conversation(&project, &session, cx))
                    .child(self.render_changes(&session, cx))
                    .child(self.render_composer(&project, cx));
            }
        }
        page.into_any_element()
    }

    /// Disarms a confirm step without doing anything.
    fn cancel_button(&self, id: impl Into<gpui_kit::ElementId>, cx: &mut Context<Self>) -> Button {
        Button::new(id)
            .label("Cancel")
            .on_click(cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.cancel_confirm(cx);
            }))
    }

    pub(super) fn render_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.apply_reset(window, cx);
        if std::mem::take(&mut self.scroll_to_end) {
            self.follow_frames = 30;
        }
        // Held for about half a second, so content that settles late is still followed.
        if self.follow_frames > 0 {
            self.follow_frames -= 1;
            self.conversation_scroll.scroll_to_bottom();
            window.request_animation_frame();
        }
        h_flex()
            .size_full()
            .debug_selector(|| "crew-view".into())
            .child(self.render_projects(cx))
            .child(self.render_detail(cx))
            .into_any_element()
    }
}

/// Small styling helpers kept next to their only users.
trait CrewStyle: Sized {
    fn when_selected(self, selected: bool, cx: &App) -> Self;
}

impl<E: Styled> CrewStyle for E {
    fn when_selected(self, selected: bool, cx: &App) -> Self {
        if selected {
            self.bg(cx.theme().accent)
        } else {
            self
        }
    }
}
