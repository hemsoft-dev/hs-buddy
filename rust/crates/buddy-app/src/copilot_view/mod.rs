//! Native assistant tools retain their own requests and selection state.
pub(crate) mod chat;
mod enterprise;
pub(crate) mod markdown;
mod premium_badge;
mod recent;
mod results;
mod sessions;
#[cfg(test)]
mod sidebar_tests;
mod usage;

use gpui_kit::assets::IconName;
use gpui_kit::base::Selectable;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement, Render,
    StatefulInteractiveElement as _, Styled, Window, div,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pane {
    Chat,
    Results,
    Sessions,
    Usage,
}

pub struct CopilotView {
    pane: Pane,
    chat: Entity<chat::ChatView>,
    results: Entity<results::ResultsView>,
    sessions: Entity<sessions::SessionsView>,
    usage: Entity<usage::UsageView>,
    /// Electron's sidebar "Recent Results" section starts collapsed.
    recent_expanded: bool,
}
impl CopilotView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let chat = cx.new(|cx| chat::ChatView::new(window, cx));
        cx.subscribe(&chat, |this, _, event: &chat::ChatEvent, cx| match event {
            chat::ChatEvent::OpenResult(id) => this.open_result(id.clone(), cx),
            chat::ChatEvent::RecentChanged => cx.notify(),
        })
        .detach();
        let results = cx.new(|cx| results::ResultsView::new(window, cx));
        cx.subscribe(&results, |this, _, _: &results::ResultsEvent, cx| {
            this.chat.update(cx, |chat, cx| chat.refresh_recent(cx))
        })
        .detach();
        let sessions = cx.new(|cx| sessions::SessionsView::new(window, cx));
        cx.observe(&results, |_, _, cx| cx.notify()).detach();
        cx.observe(&sessions, |_, _, cx| cx.notify()).detach();
        Self {
            pane: Pane::Chat,
            chat,
            results,
            sessions,
            usage: cx.new(|cx| usage::UsageView::new(window, cx)),
            recent_expanded: false,
        }
    }

    /// A Recent row, in the prompt box or the sidebar: the result's tab.
    fn open_result(&mut self, id: String, cx: &mut Context<Self>) {
        self.select(Pane::Results, cx);
        self.results
            .update(cx, |results, cx| results.inspect(id, cx));
    }
    pub fn set_default_account(&mut self, account: Option<String>, cx: &mut Context<Self>) {
        self.chat
            .update(cx, |chat, cx| chat.set_default_account(account, cx));
    }
    pub fn prepare_review(
        &mut self,
        client: buddy_core::github::GitHubClient,
        pull: buddy_core::github::PullRef,
        cx: &mut Context<Self>,
    ) {
        self.select(Pane::Chat, cx);
        self.chat
            .update(cx, |chat, cx| chat.prepare_review(client, pull, cx));
    }
    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        self.chat.update(cx, |chat, cx| {
            chat.set_active(active && self.pane == Pane::Chat, cx);
            // The sidebar lists Recent beside every pane; Chat reads it itself.
            if active && self.pane != Pane::Chat {
                chat.refresh_recent(cx);
            }
        });
        if active && self.pane == Pane::Results {
            self.results.update(cx, |results, cx| results.activate(cx));
        }
        if active && self.pane == Pane::Usage {
            self.usage.update(cx, |usage, cx| usage.activate(cx));
        }
    }
    #[cfg(test)]
    pub(crate) fn showing_usage(&self) -> bool {
        self.pane == Pane::Usage
    }

    /// `copilot-usage`: the dashboard's Open Usage action.
    pub(crate) fn show_usage(&mut self, cx: &mut Context<Self>) {
        self.select(Pane::Usage, cx);
    }

    fn select(&mut self, pane: Pane, cx: &mut Context<Self>) {
        self.pane = pane;
        self.set_active(true, cx);
        cx.notify();
    }
}
impl Render for CopilotView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut tabs = h_flex().flex_wrap().gap_2();
        for (index, (name, pane)) in [
            ("Chat", Pane::Chat),
            ("Saved results", Pane::Results),
            ("Usage", Pane::Usage),
            ("Sessions", Pane::Sessions),
        ]
        .into_iter()
        .enumerate()
        {
            tabs = tabs.child(
                Button::new(("copilot-pane", index))
                    .label(name)
                    .selected(self.pane == pane)
                    .on_click(cx.listener(move |this, _, _, cx| this.select(pane, cx))),
            );
        }
        let body = div().flex_1().min_h_0().w_full();
        let body = match self.pane {
            Pane::Chat => body.child(self.chat.clone()),
            Pane::Results => body.child(self.results.clone()),
            Pane::Sessions => body.child(self.sessions.clone()),
            Pane::Usage => body.child(self.usage.clone()),
        };
        v_flex().size_full().p_4().gap_3().child(tabs).child(body)
    }
}

impl CopilotView {
    fn toggle_recent(&mut self, cx: &mut Context<Self>) {
        self.recent_expanded = !self.recent_expanded;
        // Electron's list is live; native reads it again when it is opened.
        if self.recent_expanded {
            self.chat.update(cx, |chat, cx| chat.refresh_recent(cx));
        }
        cx.notify();
    }

    /// Electron's sidebar "Recent Results" section: the 15 newest results,
    /// each with its status icon, label and age. A row opens its result.
    pub(crate) fn sidebar(&mut self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let muted = cx.theme().muted_foreground;
        let primary = cx.theme().primary;
        let mut list = v_flex()
            .id("copilot-sidebar")
            .size_full()
            .p_2()
            .gap_1()
            .overflow_y_scroll()
            .child(
                Button::new("copilot-sidebar-recent")
                    .ghost()
                    .w_full()
                    .icon(if self.recent_expanded {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    })
                    .label("Recent Results")
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_recent(cx))),
            );
        if !self.recent_expanded {
            return list.into_any_element();
        }
        let shown = (self.pane == Pane::Results)
            .then(|| self.results.read(cx).selected_id().map(str::to_owned))
            .flatten();
        let recent = self.chat.read(cx).recent();
        if recent.rows.is_empty() {
            list = list.child(div().px_2().text_color(muted).child("No results yet"));
        }
        if recent.unavailable {
            list = list.child(
                div()
                    .px_2()
                    .text_color(muted)
                    .child("Recent results could not be refreshed."),
            );
        }
        let now = recent::now_ms();
        let mut rows = Vec::new();
        for (index, row) in recent.rows.iter().enumerate() {
            let id = row.id.clone();
            let label = recent::sidebar_label(row);
            let age = recent::age(row, now);
            let color = match row.status {
                buddy_core::copilot_results::Status::Pending => gpui_kit::rgb(0xe89b3c).into(),
                buddy_core::copilot_results::Status::Completed => gpui_kit::rgb(0x4ec9b0).into(),
                buddy_core::copilot_results::Status::Failed => gpui_kit::rgb(0xe85d5d).into(),
                _ => primary,
            };
            rows.push(
                Button::new(("copilot-sidebar-result", index))
                    .ghost()
                    .w_full()
                    .selected(shown.as_deref() == Some(row.id.as_str()))
                    .accessibility_label(format!("{label}, {age}"))
                    .tooltip(row.prompt_preview.clone())
                    .child(
                        h_flex()
                            .w_full()
                            .min_w_0()
                            .gap_2()
                            .items_center()
                            .children(
                                recent::status_icon(row.status)
                                    .map(|icon| Icon::new(icon).xsmall().text_color(color)),
                            )
                            .child(div().flex_1().min_w_0().truncate().child(label))
                            .child(div().flex_none().text_color(muted).child(age)),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.open_result(id.clone(), cx))),
            );
        }
        list = list.children(rows);
        if recent.sidebar_partial {
            list = list.child(div().px_2().text_color(muted).child(
                "Some recent results were too large to list here. Saved results has them all.",
            ));
        }
        list.into_any_element()
    }

    pub(crate) fn destination(
        &self,
        entity: gpui_kit::Entity<Self>,
        cx: &gpui_kit::App,
    ) -> crate::shell::navigation::Destination {
        let pane = self.pane;
        let child = match pane {
            Pane::Results => Some(self.results.read(cx).destination(self.results.clone())),
            Pane::Sessions => Some(self.sessions.read(cx).destination(self.sessions.clone())),
            _ => None,
        };
        let mut destination =
            crate::shell::navigation::Destination::section(Some(crate::app::Section::Copilot));
        destination.key = child
            .as_ref()
            .map(|d| d.key.clone())
            .unwrap_or_else(|| format!("copilot:{}", pane as usize));
        destination.label = child.as_ref().map(|d| d.label.clone()).unwrap_or_else(|| {
            match pane {
                Pane::Chat => "Copilot: Prompt",
                _ => "Copilot: Usage",
            }
            .into()
        });
        destination.restore = std::rc::Rc::new(move |window, cx| {
            entity.update(cx, |view, cx| view.select(pane, cx));
            if let Some(child) = &child {
                (child.restore)(window, cx);
            }
        });
        destination
    }
}
