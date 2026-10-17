use super::markdown;
use crate::runtime::Runtime;
use crate::zoom::zpx;
use buddy_core::copilot_usage::enterprise::{self, Snapshot};
use gpui_kit::base::Disableable;
use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::{
    AppContext as _, ClipboardItem, Context, Entity, InteractiveElement, IntoElement,
    ParentElement, PathPromptOptions, Render, StatefulInteractiveElement, Styled, Window, div,
};
use std::path::PathBuf;

const PAGE: usize = 20;
pub(super) struct EnterpriseView {
    path: Entity<InputState>,
    filter: Entity<InputState>,
    pending: Option<PathBuf>,
    loaded: Option<PathBuf>,
    snapshot: Option<Snapshot>,
    selected: Option<usize>,
    page: usize,
    busy: bool,
    error: Option<String>,
}
impl EnterpriseView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let path = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(
                    enterprise::default_path()
                        .map(|path| path.display().to_string())
                        .unwrap_or_default(),
                )
                .placeholder("Absolute enterprise metrics JSON path")
        });
        let filter =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search enterprise users"));
        cx.subscribe(&filter, |this, _, _: &InputEvent, cx| {
            this.page = 0;
            cx.notify();
        })
        .detach();
        cx.subscribe(&path, |_, _, _: &InputEvent, cx| cx.notify())
            .detach();
        Self {
            path,
            filter,
            pending: None,
            loaded: None,
            snapshot: None,
            selected: None,
            page: 0,
            busy: false,
            error: None,
        }
    }
    fn choose(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose enterprise metrics JSON".into()),
        });
        cx.spawn(async move |this, cx| {
            let result = rx.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(Ok(Some(paths))) => this.pending = paths.into_iter().next(),
                    Ok(Ok(None)) => {}
                    _ => this.error = Some(
                        "The file chooser is unavailable. Enter the absolute JSON path instead."
                            .into(),
                    ),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
    fn load(&mut self, cx: &mut Context<Self>) {
        if self.busy || !cx.has_global::<Runtime>() {
            return;
        }
        let path = PathBuf::from(self.path.read(cx).value().trim());
        self.busy = true;
        self.error = None;
        let rx = Runtime::global(cx).spawn(async move {
            tokio::task::spawn_blocking(move || {
                enterprise::read(&path).map(|snapshot| (path, snapshot))
            })
            .await
        });
        cx.spawn(async move |this, cx| {
            let result = rx.await;
            this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(Ok(Ok((path, snapshot)))) => {
                        this.loaded = Some(path);
                        this.snapshot = Some(snapshot);
                        this.selected = None;
                        this.page = 0;
                    }
                    Ok(Ok(Err(error))) => this.error = Some(error.into()),
                    _ => {
                        this.error =
                            Some("The metrics reader stopped. Try loading the file again.".into())
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
impl Render for EnterpriseView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(path) = self.pending.take() {
            self.path.update(cx, |state, cx| {
                state.set_value(path.display().to_string(), window, cx)
            });
        }
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let danger = theme.danger;
        let mut view=v_flex().gap_3().child(div().text_lg().child("Enterprise users"))
            .child(div().text_color(muted).child("Read a local CodexBar Copilot metrics export. This file can cover a different organization and period from the selected usage account."))
            .child(h_flex().flex_wrap().gap_2().child(div().flex_1().min_w(zpx(220.0)).child(Input::new(&self.path).disabled(self.busy)))
                .child(Button::new("copilot-enterprise-choose").label("Choose metrics file…").disabled(self.busy).on_click(cx.listener(|this,_,_,cx|this.choose(cx))))
                .child(Button::new("copilot-enterprise-load").label(if self.busy {"Reading metrics…"}else{"Load metrics file"}).disabled(self.busy).on_click(cx.listener(|this,_,_,cx|this.load(cx)))))
            .child(Input::new(&self.filter));
        if let Some(snapshot) = &self.snapshot {
            view = view.child(format!(
                "{} / {} · {} users · {} active · Updated {}",
                snapshot.enterprise,
                snapshot.organization,
                snapshot.users.len(),
                snapshot
                    .users
                    .iter()
                    .filter(|user| user.gross_quantity > 0.0)
                    .count(),
                snapshot.generated_at
            ));
            if let (Some(year), Some(month)) = (snapshot.year, snapshot.month) {
                view = view.child(div().text_color(muted).child(format!(
                    "Export period {year:.0}-{month:02.0} · {} reported days",
                    snapshot.days.len()
                )));
            }
            let search = self.filter.read(cx).value().to_lowercase();
            let rows: Vec<_> = snapshot
                .users
                .iter()
                .enumerate()
                .filter(|(_, user)| user.login.to_lowercase().contains(&search))
                .collect();
            if rows.is_empty() {
                view = view.child("No enterprise users match this search.");
            }
            for (index, user) in rows.iter().skip(self.page * PAGE).take(PAGE) {
                let index = *index;
                view = view.child(
                    Button::new(("copilot-enterprise-user", index))
                        .label(format!(
                            "{} · {:.0} credits · ${:.2} gross · {} models · {}{}",
                            user.login,
                            user.gross_quantity,
                            user.gross_amount,
                            user.models,
                            user.top_model.as_deref().unwrap_or("No model usage"),
                            if user.success { "" } else { " · Failed" }
                        ))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.selected = Some(index);
                            cx.notify();
                        })),
                );
            }
            view = view.child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("copilot-enterprise-prev")
                            .label("Previous users")
                            .disabled(self.page == 0)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.page = this.page.saturating_sub(1);
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("copilot-enterprise-next")
                            .label("Next users")
                            .disabled((self.page + 1) * PAGE >= rows.len())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.page += 1;
                                cx.notify();
                            })),
                    ),
            );
            if let Some(user) = self.selected.and_then(|index| snapshot.users.get(index)) {
                view = view
                    .child(div().text_lg().child(user.login.clone()))
                    .child(format!(
                        "{:.0} credits · ${:.2} gross · ${:.2} net · Top model {:.0} credits",
                        user.gross_quantity,
                        user.gross_amount,
                        user.net_amount,
                        user.top_model_quantity
                    ))
                    .child(
                        Button::new("copilot-enterprise-copy-json")
                            .label("Copy user source JSON")
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(user) = this.snapshot.as_ref().and_then(|snapshot| {
                                    this.selected.and_then(|index| snapshot.users.get(index))
                                }) {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        user.source_json.clone(),
                                    ));
                                }
                            })),
                    )
                    .child(
                        div()
                            .id("copilot-enterprise-source-json")
                            .max_h(zpx(300.0))
                            .overflow_y_scroll()
                            .child(markdown::response(
                                "copilot-enterprise-json",
                                format!("```json\n{}\n```", user.source_json),
                            )),
                    );
                if let Some(error) = &user.error {
                    view = view.child(div().text_color(danger).child(error.clone()));
                }
            }
        }
        if let Some(path) = &self.loaded {
            view = view.child(
                div()
                    .text_color(muted)
                    .child(format!("Loaded metrics source: {}", path.display())),
            );
        }
        if let Some(error) = &self.error {
            view = view.child(div().text_color(danger).child(error.clone()));
        }
        view
    }
}
