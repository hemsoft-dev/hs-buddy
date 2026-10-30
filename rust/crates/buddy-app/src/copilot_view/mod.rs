//! Native assistant tools retain their own requests and selection state.
pub(crate) mod chat;
mod enterprise;
pub(crate) mod markdown;
mod results;
mod sessions;
mod usage;

use gpui_kit::base::Selectable;
use gpui_kit::component::button::Button;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, ParentElement, Render, Styled, Window, div,
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
}
impl CopilotView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let chat = cx.new(|cx| chat::ChatView::new(window, cx));
        cx.subscribe(&chat, |this, _, event: &chat::ChatEvent, cx| {
            let chat::ChatEvent::OpenResult(id) = event;
            this.select(Pane::Results, cx);
            this.results
                .update(cx, |results, cx| results.inspect(id.clone(), cx));
        })
        .detach();
        let results = cx.new(|cx| results::ResultsView::new(window, cx));
        let sessions = cx.new(|cx| sessions::SessionsView::new(window, cx));
        cx.observe(&results, |_, _, cx| cx.notify()).detach();
        cx.observe(&sessions, |_, _, cx| cx.notify()).detach();
        Self {
            pane: Pane::Chat,
            chat,
            results,
            sessions,
            usage: cx.new(|cx| usage::UsageView::new(window, cx)),
        }
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
            chat.set_active(active && self.pane == Pane::Chat, cx)
        });
        if active && self.pane == Pane::Results {
            self.results.update(cx, |results, cx| results.activate(cx));
        }
        if active && self.pane == Pane::Usage {
            self.usage.update(cx, |usage, cx| usage.activate(cx));
        }
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
