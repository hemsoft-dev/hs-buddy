//! Session files are opened only after an explicit source selection.
use std::path::PathBuf;
use std::sync::Arc;

use buddy_core::copilot_sessions::sources::{self, Platform, Source};
use buddy_core::copilot_sessions::storage::{ScanLimits, ScanResult, SessionRoot};
use buddy_core::copilot_sessions::{Digest, Limits, SessionDetail};
use gpui_kit::base::{Disableable, Selectable};
use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::{
    AppContext as _, Context, Entity, InteractiveElement, IntoElement, ParentElement,
    PathPromptOptions, Render, StatefulInteractiveElement, Styled, Window, div,
};

use super::markdown;
use crate::runtime::Runtime;
use crate::zoom::zpx;

const PAGE: usize = 20;

#[cfg(test)]
#[path = "sessions_tests.rs"]
mod tests;

pub(super) struct SessionsView {
    path: Entity<InputState>,
    sources: Vec<Source>,
    root: Option<Arc<SessionRoot>>,
    loaded_source: Option<PathBuf>,
    scan: ScanResult,
    page: usize,
    selected: Option<usize>,
    detail: Option<SessionDetail>,
    digest: Option<Digest>,
    request_page: usize,
    busy: bool,
    error: Option<String>,
}
impl SessionsView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let path =
            cx.new(|cx| InputState::new(window, cx).placeholder("Absolute VS Code User directory"));
        cx.subscribe(&path, |_, _, _: &InputEvent, cx| cx.notify())
            .detach();
        let platform = if cfg!(windows) {
            Platform::Windows
        } else if cfg!(target_os = "macos") {
            Platform::Mac
        } else {
            Platform::Linux
        };
        let home =
            std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from);
        let app_data = std::env::var_os("APPDATA").map(PathBuf::from);
        let xdg = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from);
        Self {
            path,
            sources: sources::standard_sources(
                platform,
                home.as_deref(),
                app_data.as_deref(),
                xdg.as_deref(),
            ),
            root: None,
            loaded_source: None,
            scan: ScanResult::default(),
            page: 0,
            selected: None,
            detail: None,
            digest: None,
            request_page: 0,
            busy: false,
            error: None,
        }
    }
    fn choose(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose VS Code User directory".into()),
        });
        cx.spawn(async move |this, cx| {
            let result=rx.await;
            this.update(cx, |this,cx| {
                match result {
                    Ok(Ok(Some(paths))) => if let Some(path)=paths.into_iter().next() {this.load(path,cx);},
                    Ok(Ok(None))=>{},
                    _=>{this.error=Some("The folder chooser is unavailable. Enter an absolute User directory instead.".into());cx.notify();}
                }
            }).ok();
        }).detach();
    }
    fn load(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.busy || !cx.has_global::<Runtime>() {
            return;
        }
        if !path.is_absolute() {
            self.error = Some("Enter an absolute VS Code User directory.".into());
            cx.notify();
            return;
        }
        self.busy = true;
        self.error = None;
        let rx = Runtime::global(cx).spawn(async move {
            tokio::task::spawn_blocking(move || {
                let root = Arc::new(SessionRoot::open(&path)?);
                let scan = root.scan(ScanLimits::default())?;
                Ok::<_, buddy_core::copilot_sessions::storage::StorageError>((root, scan, path))
            })
            .await
        });
        cx.spawn(async move |this, cx| {
            let result = rx.await;
            this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(Ok(Ok((root, scan, path)))) => {
                        this.loaded_source = Some(path);
                        this.root = Some(root);
                        this.scan = scan;
                        this.page = 0;
                        this.selected = None;
                        this.detail = None;
                        this.digest = None;
                        this.request_page = 0;
                    }
                    Ok(Ok(Err(error))) => this.error = Some(error.to_string()),
                    _ => {
                        this.error = Some("The session reader stopped. Try scanning again.".into())
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
    fn open(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.busy || !cx.has_global::<Runtime>() {
            return;
        }
        let (Some(root), Some(summary)) = (self.root.clone(), self.scan.sessions.get(index)) else {
            return;
        };
        let key = summary.file.clone();
        let workspace = summary.workspace_name.clone();
        let agent = summary.agent.clone();
        self.busy = true;
        self.error = None;
        self.selected = Some(index);
        self.detail = None;
        self.digest = None;
        self.request_page = 0;
        let rx = Runtime::global(cx).spawn(async move {
            tokio::task::spawn_blocking(move || {
                root.detail(&key, Limits::default()).map(|detail| {
                    let digest =
                        detail.digest(&workspace, &agent, chrono::Utc::now().timestamp_millis());
                    (detail, digest)
                })
            })
            .await
        });
        cx.spawn(async move |this, cx| {
            let result = rx.await;
            this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(Ok(Ok((detail, digest)))) => {
                        this.detail = Some(detail);
                        this.digest = Some(digest);
                    }
                    Ok(Ok(Err(error))) => this.error = Some(error.to_string()),
                    _ => {
                        this.error =
                            Some("The session reader stopped. Select the session again.".into())
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
}
impl Render for SessionsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let danger = theme.danger;
        let mut sources = h_flex().flex_wrap().gap_2();
        for (index, source) in self.sources.iter().enumerate() {
            let path = source.user_directory.clone();
            sources = sources.child(
                Button::new(("copilot-session-source", index))
                    .label(format!("Scan {}", source.name))
                    .disabled(self.busy)
                    .on_click(cx.listener(move |this, _, _, cx| this.load(path.clone(), cx))),
            );
        }
        let mut list = v_flex()
            .id("copilot-session-list")
            .w(zpx(260.0))
            .h_full()
            .flex_shrink_0()
            .min_h_0()
            .gap_1()
            .overflow_y_scroll();
        for (index, session) in self
            .scan
            .sessions
            .iter()
            .enumerate()
            .skip(self.page * PAGE)
            .take(PAGE)
        {
            let title = if !session.title.is_empty() {
                session.title.clone()
            } else if !session.first_prompt.is_empty() {
                session.first_prompt.clone()
            } else {
                format!("Session {}", session.session_id)
            };
            list = list.child(
                Button::new(("copilot-session-row", index))
                    .label(format!("{} · {}", session.workspace_name, title))
                    .w_full()
                    .selected(self.selected == Some(index))
                    .disabled(self.busy)
                    .on_click(cx.listener(move |this, _, _, cx| this.open(index, cx))),
            );
        }
        let mut detail = v_flex()
            .id("copilot-session-detail")
            .debug_selector(|| "session-detail-viewport".into())
            .flex_1()
            .min_w_0()
            .min_h_0()
            .gap_3()
            .overflow_y_scroll();
        let mut request_paging = None;
        if let (Some(session), Some(digest)) = (&self.detail, &self.digest) {
            detail=detail.child(div().text_lg().child(if session.title.is_empty() {"Session details".into()} else {session.title.clone()}))
                .child(format!("{} requests · {} prompt tokens · {} output tokens · {} tool calls · {:.1}s",digest.request_count,digest.total_prompt_tokens,digest.total_output_tokens,digest.total_tool_calls,digest.total_duration_ms/1000.0))
                .child(div().text_color(muted).child(format!("{} · {}",digest.workspace_name,digest.model)))
                .child(format!("Output / prompt tokens: {:.2} · Tools per request: {:.2} · Requests using search: {}",digest.token_efficiency,digest.tool_density,digest.search_churn))
                .child(div().text_color(muted).child(format!("Legacy cost estimate: ${:.4}. This approximation is not a provider invoice or current token price.",digest.estimated_cost)));
            if digest.partial {
                detail=detail.child(div().text_color(danger).child("Some session records could not be parsed. These metrics describe the readable portion."));
            }
            for (index, request) in session
                .results
                .iter()
                .enumerate()
                .skip(self.request_page * PAGE)
                .take(PAGE)
            {
                detail = detail
                    .child(
                        div()
                            .debug_selector(move || format!("session-request-{index}"))
                            .text_color(muted)
                            .child(format!(
                                "Request {} · {} → {} tokens · {} tool calls · {:.1}s",
                                index + 1,
                                request.prompt_tokens,
                                request.output_tokens,
                                request.tool_call_count,
                                request.total_elapsed_ms / 1000.0
                            )),
                    )
                    .child(markdown::response(
                        ("copilot-session-prompt", index),
                        request.prompt.clone(),
                    ));
                if !request.tool_names.is_empty() {
                    detail = detail.child(format!("Tools: {}", request.tool_names.join(", ")));
                }
            }
            request_paging = Some(
                h_flex()
                    .debug_selector(|| "session-request-pager".into())
                    .flex_shrink_0()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        Button::new("copilot-session-previous-requests")
                            .label("Previous requests")
                            .disabled(self.request_page == 0 || self.busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.request_page = this.request_page.saturating_sub(1);
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("copilot-session-next-requests")
                            .label("Next requests")
                            .disabled(
                                (self.request_page + 1) * PAGE >= session.results.len()
                                    || self.busy,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.request_page += 1;
                                cx.notify();
                            })),
                    ),
            );
        } else {
            detail = detail.child(
                div()
                    .text_color(muted)
                    .child("Select a session to inspect its requests and efficiency metrics."),
            );
        }
        let mut detail_panel = v_flex()
            .flex_1()
            .h_full()
            .min_w_0()
            .min_h_0()
            .gap_3()
            .overflow_hidden()
            .child(detail);
        if let Some(paging) = request_paging {
            detail_panel = detail_panel.child(paging);
        }
        let mut view = v_flex()
            .size_full()
            .min_h_0()
            .overflow_hidden()
            .gap_3()
            .child(sources)
            .child("VS Code User directory")
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&self.path).disabled(self.busy)),
                    )
                    .child(
                        Button::new("copilot-session-choose")
                            .label("Choose folder…")
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, _, cx| this.choose(cx))),
                    )
                    .child(
                        Button::new("copilot-session-scan")
                            .label("Scan folder")
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                let path = PathBuf::from(this.path.read(cx).value().trim());
                                this.load(path, cx);
                            })),
                    ),
            )
            .child(div().text_color(muted).child(if self.busy {
                "Reading session files…".into()
            } else {
                format!(
                    "{} sessions · {} skipped{}",
                    self.scan.sessions.len(),
                    self.scan.skipped,
                    if self.scan.truncated {
                        " · Scan limit reached; the list is incomplete"
                    } else {
                        ""
                    }
                )
            }))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("copilot-session-previous")
                            .label("Previous sessions")
                            .disabled(self.page == 0 || self.busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.page = this.page.saturating_sub(1);
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("copilot-session-next")
                            .label("Next sessions")
                            .disabled(
                                (self.page + 1) * PAGE >= self.scan.sessions.len() || self.busy,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.page += 1;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_start()
                    .overflow_hidden()
                    .gap_4()
                    .child(list)
                    .child(detail_panel),
            );
        if let Some(source) = &self.loaded_source {
            view = view.child(
                div()
                    .debug_selector(|| "session-loaded-source".into())
                    .flex_shrink_0()
                    .w_full()
                    .min_w_0()
                    .text_color(muted)
                    .child(format!("Loaded source: {}", source.display())),
            );
        }
        if let Some(error) = &self.error {
            view = view.child(
                div()
                    .debug_selector(|| "session-source-error".into())
                    .flex_shrink_0()
                    .w_full()
                    .min_w_0()
                    .text_color(danger)
                    .child(error.clone()),
            );
        }
        view
    }
}
