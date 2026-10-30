//! Bottom-panel sessions are independent of the full terminal workspace.
use super::grid::{GridEvent, TerminalGrid};
use crate::{runtime::Runtime, settings::Settings, zoom::zpx};
use buddy_core::terminal::{Session, ShellSpec, TerminalSize};
use buddy_core::terminal_backend::TerminalBackend;
use buddy_core::terminal_panel::{PanelTab, parse_tabs};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{ActiveTheme, Selectable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    AppContext as _, Context, Entity, EventEmitter, InteractiveElement, IntoElement, ParentElement,
    Render, StatefulInteractiveElement, Styled, Window, div,
};
use std::{path::PathBuf, sync::Arc, time::Duration};

struct Tab {
    id: String,
    metadata: PanelTab,
    grid: Option<Entity<TerminalGrid>>,
    starting: bool,
    error: Option<String>,
}

#[derive(Clone)]
struct DraggedTab {
    id: String,
    title: String,
}
impl Render for DraggedTab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .p_2()
            .bg(cx.theme().background)
            .child(self.title.clone())
    }
}

pub struct TerminalPanel {
    tabs: Vec<Tab>,
    selected: Option<String>,
    visible: bool,
    focus_grid: bool,
    focus_controls: bool,
    controls_focus: gpui_kit::FocusHandle,
    rename: Entity<InputState>,
    renaming: bool,
    backend: Option<TerminalBackend>,
    loaded: bool,
    loading: bool,
    generation: u64,
    saving: bool,
    error: Option<String>,
    cache_key: String,
    remote: Option<Vec<PanelTab>>,
    read_ok: bool,
    invalid_cache: bool,
}
impl EventEmitter<crate::explorer_view::OpenPath> for TerminalPanel {}

impl TerminalPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let backend = TerminalBackend::new(&buddy_core::convex_data::convex_url()).ok();
        let cache_key = format!("terminalPanel:{}", buddy_core::convex_data::convex_url());
        let cached = Settings::global(cx)
            .config
            .native
            .extra
            .get(&cache_key)
            .cloned();
        let parsed = cached.clone().map(parse_tabs).transpose();
        let loaded = false;
        let invalid_cache = parsed.is_err();
        let error = parsed.as_ref().err().map(ToString::to_string);
        let tabs = parsed
            .unwrap_or_default()
            .unwrap_or_default()
            .into_iter()
            .map(Self::tab)
            .collect::<Vec<_>>();
        let selected = tabs.first().map(|tab| tab.id.clone());
        let rename = cx.new(|cx| InputState::new(window, cx).placeholder("Terminal name"));
        cx.subscribe(&rename, |this, _, event, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.finish_rename(cx);
            }
        })
        .detach();
        Self {
            tabs,
            selected,
            visible: false,
            focus_grid: false,
            focus_controls: false,
            controls_focus: cx.focus_handle(),
            rename,
            renaming: false,
            backend,
            loaded,
            loading: false,
            generation: 0,
            saving: false,
            error,
            cache_key,
            remote: None,
            read_ok: false,
            invalid_cache,
        }
    }

    fn tab(metadata: PanelTab) -> Tab {
        Tab {
            id: uuid::Uuid::new_v4().to_string(),
            metadata,
            grid: None,
            starting: false,
            error: None,
        }
    }

    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        self.focus_grid = visible;
        if visible && !self.loaded && !self.loading {
            self.load(cx);
        }
        if visible && self.loaded && self.tabs.is_empty() && self.error.is_none() {
            self.add(None, cx);
        }
        self.update_visibility(cx);
        if visible {
            self.start_selected(cx);
        }
        cx.notify();
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        let Some(backend) = self.backend.clone().filter(|_| cx.has_global::<Runtime>()) else {
            return;
        };
        self.loading = true;
        let generation = self.generation;
        let receiver = Runtime::global(cx).spawn(async move { backend.load_panel().await });
        cx.spawn(async move |this, cx| {
            if let Ok(result) = receiver.await {
                this.update(cx, |this, cx| {
                    this.loading = false;
                    this.loaded = true;
                    match result {
                        Ok((tabs, height)) => {
                            if this.invalid_cache {
                                this.error = Some("Local terminal metadata has an unsupported format. Original configuration retained; restore it before editing tabs.".into());
                                cx.notify();
                                return;
                            }
                            this.read_ok = true;
                            if !this.reconcile(tabs, generation != this.generation) {
                                this.error = None;
                                cx.notify();
                                return;
                            }
                            if generation == this.generation && let Some(height) = height {
                                Settings::update(cx, |config| {
                                    config.ui.terminal_panel_height = height
                                });
                            }
                            this.error = None;
                            this.cache_metadata(cx);
                            if this.tabs.is_empty() && this.visible {
                                this.add(None, cx);
                            }
                            this.update_visibility(cx);
                            this.start_selected(cx);
                        }
                        Err(error) => {
                            this.error = Some(format!(
                                "{error} Retry sync reloads saved tabs."
                            ));
                            if this.tabs.is_empty() && this.visible && !this.invalid_cache {
                                this.add(None, cx);
                            }
                        }
                    }
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    fn reconcile(&mut self, tabs: Vec<PanelTab>, edited: bool) -> bool {
        if (edited || !self.tabs.is_empty()) && self.metadata() != tabs {
            self.remote = Some(tabs);
            return false;
        }
        if self.tabs.is_empty() {
            self.tabs = tabs.into_iter().map(Self::tab).collect();
            self.selected = self.tabs.first().map(|tab| tab.id.clone());
        }
        true
    }

    pub fn add(&mut self, context: Option<PanelTab>, cx: &mut Context<Self>) {
        if self.invalid_cache || self.tabs.len() >= 256 {
            return;
        }
        if let Some(slug) = context.as_ref().and_then(|tab| tab.repo_slug.as_ref())
            && let Some(tab) = self
                .tabs
                .iter()
                .find(|tab| tab.metadata.repo_slug.as_ref() == Some(slug))
        {
            self.selected = Some(tab.id.clone());
        } else {
            let metadata = context.unwrap_or_else(|| PanelTab {
                title: format!("Terminal {}", self.tabs.len() + 1),
                cwd: String::new(),
                repo_slug: None,
                color: None,
            });
            let tab = Self::tab(metadata);
            self.selected = Some(tab.id.clone());
            self.tabs.push(tab);
            self.changed(cx);
        }
        self.focus_grid = true;
        self.update_visibility(cx);
        self.start_selected(cx);
        cx.notify();
    }

    fn metadata(&self) -> Vec<PanelTab> {
        self.tabs.iter().map(|tab| tab.metadata.clone()).collect()
    }

    fn cache_metadata(&mut self, cx: &mut Context<Self>) -> bool {
        let tabs = serde_json::to_value(self.metadata()).expect("serializable tabs");
        let key = self.cache_key.clone();
        let saved = Settings::update(cx, |config| {
            config.native.extra.insert(key.clone(), tabs.clone());
        });
        if !saved {
            self.error = Some("Terminal tabs could not be saved locally. Retry sync after restoring configuration access.".into());
        }
        saved
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        if self.invalid_cache {
            return;
        }
        self.generation += 1;
        let generation = self.generation;
        self.cache_metadata(cx);
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(500))
                .await;
            this.update(cx, |this, cx| {
                if this.generation == generation {
                    this.save(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        if self.saving || !self.read_ok || self.remote.is_some() || self.invalid_cache {
            return;
        }
        let Some(backend) = self.backend.clone().filter(|_| cx.has_global::<Runtime>()) else {
            return;
        };
        if !self.cache_metadata(cx) {
            return;
        }
        self.saving = true;
        let generation = self.generation;
        let tabs = self.metadata();
        let height = Settings::global(cx).config.ui.terminal_panel_height;
        let receiver =
            Runtime::global(cx).spawn(async move { backend.save_panel(tabs, height).await });
        cx.spawn(async move |this, cx| {
            if let Ok(result) = receiver.await {
                this.update(cx, |this, cx| {
                    this.saving = false;
                    match result {
                        Ok(()) => {
                            this.error = None;
                            if this.generation != generation {
                                this.save(cx);
                            }
                        }
                        Err(error) => {
                            this.error = Some(format!(
                                "{error} Local tabs retained. Retry sync to save them."
                            ))
                        }
                    }
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    pub fn height_changed(&mut self, cx: &mut Context<Self>) {
        self.changed(cx);
    }

    fn update_visibility(&self, cx: &mut Context<Self>) {
        for tab in &self.tabs {
            if let Some(grid) = &tab.grid {
                let selected = self.selected.as_ref() == Some(&tab.id);
                grid.update(cx, |grid, cx| {
                    grid.set_visibility(self.visible && selected, selected, cx)
                });
            }
        }
    }

    fn start_selected(&mut self, cx: &mut Context<Self>) {
        if !self.visible || !cx.has_global::<Runtime>() {
            return;
        }
        let Some(tab) = self
            .tabs
            .iter_mut()
            .find(|tab| Some(&tab.id) == self.selected.as_ref())
        else {
            return;
        };
        if tab.starting || tab.grid.is_some() || tab.error.is_some() {
            return;
        }
        tab.starting = true;
        let id = tab.id.clone();
        let cwd = tab
            .metadata
            .repo_slug
            .as_deref()
            .and_then(|slug| slug.split_once('/'))
            .and_then(|(owner, name)| {
                crate::ralph_view::local_repository(
                    &Settings::global(cx).config.github.accounts,
                    owner,
                    name,
                    None,
                )
            })
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_else(|| tab.metadata.cwd.clone());
        let shell_config = Settings::global(cx)
            .config
            .native
            .extra
            .get("terminalShells")
            .cloned();
        let receiver = Runtime::global(cx).spawn(async move {
            tokio::task::spawn_blocking(move || {
                let shell = ShellSpec::configured(shell_config.as_ref(), &PathBuf::from(cwd))
                    .map_err(str::to_string)?;
                let directory = shell.directory.to_string_lossy().into_owned();
                Session::spawn(
                    &std::env::current_exe().map_err(|e| e.to_string())?,
                    shell,
                    TerminalSize::new(80, 24).expect("valid size"),
                )
                .map(|session| (Arc::new(session), directory))
                .map_err(|e| e.to_string())
            })
            .await
            .map_err(|_| "Terminal startup worker stopped.".to_string())
            .and_then(|result| result)
        });
        cx.spawn(async move |this, cx| {
            if let Ok(result) = receiver.await {
                this.update(cx, |this, cx| {
                    let Some(index) = this.tabs.iter().position(|tab| tab.id == id) else {
                        return;
                    };
                    this.tabs[index].starting = false;
                    match result {
                        Ok((session, directory)) => {
                            if this.tabs[index].metadata.cwd != directory {
                                this.tabs[index].metadata.cwd = directory;
                                this.changed(cx);
                            }
                            let grid = cx.new(|cx| TerminalGrid::new(session, cx));
                            let id = id.clone();
                            cx.subscribe(&grid, move |this, _, event, cx| {
                                if matches!(event, GridEvent::FocusControls) {
                                    this.focus_controls = true;
                                }
                                if let GridEvent::WorkingDirectory(cwd) = event
                                    && let Some(tab) = this.tabs.iter_mut().find(|tab| tab.id == id)
                                {
                                    tab.metadata.cwd = cwd.clone();
                                    this.changed(cx);
                                }
                                cx.notify();
                            })
                            .detach();
                            this.tabs[index].grid = Some(grid);
                            this.focus_grid = true;
                        }
                        Err(error) => this.tabs[index].error = Some(error),
                    }
                    this.update_visibility(cx);
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    fn move_tab(&mut self, from: &str, to: &str) -> bool {
        let Some(source) = self.tabs.iter().position(|tab| tab.id == from) else {
            return false;
        };
        let Some(target) = self.tabs.iter().position(|tab| tab.id == to) else {
            return false;
        };
        if source == target {
            return false;
        }
        let tab = self.tabs.remove(source);
        self.tabs.insert(target, tab);
        true
    }

    fn remove_tab(&mut self, id: &str) -> bool {
        let Some(index) = self.tabs.iter().position(|tab| tab.id == id) else {
            return false;
        };
        self.tabs.remove(index); // Dropping the grid releases its owned PTY session.
        if self.selected.as_deref() == Some(id) {
            self.selected = self
                .tabs
                .get(index.min(self.tabs.len().saturating_sub(1)))
                .map(|tab| tab.id.clone());
        }
        true
    }

    fn close(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.remove_tab(id) {
            return;
        }
        if self.tabs.is_empty() {
            Settings::update(cx, |config| config.ui.terminal_open = false);
        }
        self.renaming = false;
        self.focus_grid = true;
        self.changed(cx);
        self.update_visibility(cx);
        self.start_selected(cx);
        cx.notify();
    }

    fn finish_rename(&mut self, cx: &mut Context<Self>) {
        let name = self.rename.read(cx).value().trim().to_string();
        if !name.is_empty()
            && name.len() <= 1024
            && let Some(tab) = self
                .tabs
                .iter_mut()
                .find(|tab| Some(&tab.id) == self.selected.as_ref())
        {
            tab.metadata.title = name;
            self.changed(cx);
        }
        self.renaming = false;
        self.focus_grid = true;
        cx.notify();
    }
}

impl Render for TerminalPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.focus_grid {
            if let Some(grid) = self
                .tabs
                .iter()
                .find(|tab| Some(&tab.id) == self.selected.as_ref())
                .and_then(|tab| tab.grid.as_ref())
            {
                grid.update(cx, |grid, cx| grid.focus(window, cx));
            }
            self.focus_grid = false;
        }
        if self.focus_controls {
            window.focus(&self.controls_focus, cx);
            self.focus_controls = false;
        }
        let mut header = h_flex()
            .track_focus(&self.controls_focus)
            .gap_1()
            .items_center()
            .px_2()
            .py_1()
            .child("TERMINAL");
        let mut tab_row = h_flex().min_w_0().flex_1().overflow_x_scrollbar();
        for tab in &self.tabs {
            let select = tab.id.clone();
            let close = tab.id.clone();
            let button = Button::new(format!("panel-tab-{}", tab.id))
                .ghost()
                .xsmall()
                .label(tab.metadata.title.clone())
                .selected(self.selected.as_ref() == Some(&tab.id));
            if let Some(color) = tab
                .metadata
                .color
                .as_deref()
                .and_then(|value| u32::from_str_radix(value.trim_start_matches('#'), 16).ok())
            {
                tab_row = tab_row.child(
                    div()
                        .size(zpx(6.0))
                        .flex_shrink_0()
                        .rounded_full()
                        .bg(gpui_kit::rgb(color)),
                );
            }
            let target = tab.id.clone();
            let dragged = DraggedTab {
                id: tab.id.clone(),
                title: tab.metadata.title.clone(),
            };
            tab_row = tab_row
                .child(
                    div()
                        .id(format!("panel-drag-{}", tab.id))
                        .on_drag(dragged, |dragged, _, _, cx| cx.new(|_| dragged.clone()))
                        .on_drop(cx.listener(move |this, dragged: &DraggedTab, _, cx| {
                            if this.move_tab(&dragged.id, &target) {
                                this.changed(cx);
                                cx.notify();
                            }
                        }))
                        .child(button.on_click(cx.listener(move |this, _, _, cx| {
                            this.selected = Some(select.clone());
                            this.renaming = false;
                            this.focus_grid = true;
                            this.update_visibility(cx);
                            this.start_selected(cx);
                            cx.notify();
                        }))),
                )
                .child(
                    Button::new(format!("panel-close-{}", tab.id))
                        .ghost()
                        .xsmall()
                        .icon(gpui_kit::assets::IconName::X)
                        .accessibility_label(format!("Close {}", tab.metadata.title))
                        .on_click(cx.listener(move |this, _, _, cx| this.close(&close, cx))),
                );
        }
        header = header
            .child(tab_row)
            .child(
                Button::new("panel-new")
                    .ghost()
                    .xsmall()
                    .icon(gpui_kit::assets::IconName::Plus)
                    .tooltip("New Terminal")
                    .accessibility_label("New Terminal")
                    .on_click(cx.listener(|this, _, _, cx| this.add(None, cx))),
            )
            .child(div().flex_1());
        let selected = self.selected.clone();
        let weak = cx.entity().downgrade();
        header = header
            .child(
                Button::new("panel-tab-actions")
                    .ghost()
                    .xsmall()
                    .label("Actions")
                    .dropdown_menu(move |mut menu, _, _| {
                        let rename = weak.clone();
                        menu = menu.item(PopupMenuItem::new("Rename terminal").on_click(
                            move |_, window, cx| {
                                rename
                                    .update(cx, |this, cx| {
                                        if let Some(tab) = this
                                            .tabs
                                            .iter()
                                            .find(|tab| Some(&tab.id) == this.selected.as_ref())
                                        {
                                            let title = tab.metadata.title.clone();
                                            this.rename.update(cx, |input, cx| {
                                                input.set_value(title, window, cx);
                                                input.focus(window, cx);
                                            });
                                            this.renaming = true;
                                            cx.notify();
                                        }
                                    })
                                    .ok();
                            },
                        ));
                        for (label, color) in [
                            ("Default color", None),
                            ("Red", Some("#e74856")),
                            ("Orange", Some("#f7a72e")),
                            ("Yellow", Some("#f9f1a5")),
                            ("Green", Some("#16c60c")),
                            ("Teal", Some("#61d6d6")),
                            ("Blue", Some("#3b78ff")),
                            ("Purple", Some("#b4009e")),
                            ("Pink", Some("#ff69b4")),
                            ("White", Some("#cccccc")),
                        ] {
                            let weak = weak.clone();
                            let id = selected.clone();
                            menu =
                                menu.item(PopupMenuItem::new(label).on_click(move |_, _, cx| {
                                    weak.update(cx, |this, cx| {
                                        if let Some(tab) = this
                                            .tabs
                                            .iter_mut()
                                            .find(|tab| Some(&tab.id) == id.as_ref())
                                        {
                                            tab.metadata.color = color.map(str::to_string);
                                            this.changed(cx);
                                            cx.notify();
                                        }
                                    })
                                    .ok();
                                }));
                        }
                        for (label, delta) in [("Move tab left", -1isize), ("Move tab right", 1)] {
                            let weak = weak.clone();
                            let id = selected.clone();
                            menu =
                                menu.item(PopupMenuItem::new(label).on_click(move |_, _, cx| {
                                    weak.update(cx, |this, cx| {
                                        if let Some(index) = this
                                            .tabs
                                            .iter()
                                            .position(|tab| Some(&tab.id) == id.as_ref())
                                        {
                                            let target = index
                                                .saturating_add_signed(delta)
                                                .min(this.tabs.len() - 1);
                                            this.tabs.swap(index, target);
                                            this.changed(cx);
                                            cx.notify();
                                        }
                                    })
                                    .ok();
                                }));
                        }
                        let restart = weak.clone();
                        menu = menu.item(PopupMenuItem::new("Restart shell").on_click(
                            move |_, _, cx| {
                                restart
                                    .update(cx, |this, cx| {
                                        if let Some(tab) = this
                                            .tabs
                                            .iter_mut()
                                            .find(|tab| Some(&tab.id) == this.selected.as_ref())
                                        {
                                            if tab.starting {
                                                return;
                                            }
                                            tab.grid = None;
                                            tab.error = None;
                                        }
                                        this.focus_grid = true;
                                        this.start_selected(cx);
                                        cx.notify();
                                    })
                                    .ok();
                            },
                        ));
                        let folder = weak.clone();
                        menu = menu.item(PopupMenuItem::new("Open folder view").on_click(
                            move |_, _, cx| {
                                folder
                                    .update(cx, |this, cx| {
                                        if let Some(tab) = this
                                            .tabs
                                            .iter()
                                            .find(|tab| Some(&tab.id) == this.selected.as_ref())
                                            && !tab.metadata.cwd.is_empty()
                                        {
                                            cx.emit(crate::explorer_view::OpenPath(PathBuf::from(
                                                &tab.metadata.cwd,
                                            )));
                                        }
                                    })
                                    .ok();
                            },
                        ));
                        menu
                    })
                    .on_open_change(|open, _, cx| crate::browser_view::Overlays::set(*open, cx)),
            )
            .child(
                Button::new("panel-hide")
                    .ghost()
                    .xsmall()
                    .icon(gpui_kit::assets::IconName::X)
                    .tooltip("Hide Terminal")
                    .accessibility_label("Hide Terminal")
                    .on_click(|_, window, cx| {
                        window.dispatch_action(Box::new(crate::app::ToggleTerminal), cx);
                    }),
            );
        let mut root = v_flex()
            .size_full()
            .min_h_0()
            .border_t_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().background)
            .child(header);
        if self.remote.is_some() {
            root = root.child(
                h_flex()
                    .gap_2()
                    .px_2()
                    .child("Saved tabs differ. Running shells are retained until you choose.")
                    .child(
                        Button::new("panel-keep-local")
                            .xsmall()
                            .label("Keep local tabs")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.remote = None;
                                this.changed(cx);
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("panel-use-saved")
                            .xsmall()
                            .label("Use saved tabs")
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(tabs) = this.remote.take() {
                                    this.tabs = tabs.into_iter().map(Self::tab).collect();
                                    this.selected = this.tabs.first().map(|tab| tab.id.clone());
                                    this.changed(cx);
                                    this.update_visibility(cx);
                                    this.start_selected(cx);
                                    cx.notify();
                                }
                            })),
                    ),
            );
        }
        if self.renaming {
            root = root.child(
                h_flex().gap_2().child(Input::new(&self.rename)).child(
                    Button::new("panel-rename-save")
                        .label("Save name")
                        .on_click(cx.listener(|this, _, _, cx| this.finish_rename(cx))),
                ),
            );
        }
        if let Some(error) = &self.error {
            root = root.child(
                h_flex()
                    .gap_2()
                    .px_2()
                    .child(div().flex_1().min_w_0().child(error.clone()))
                    .child(
                        Button::new("panel-sync")
                            .xsmall()
                            .label("Retry sync")
                            .on_click(cx.listener(|this, _, _, cx| {
                                if !this.read_ok {
                                    this.load(cx);
                                } else {
                                    this.save(cx);
                                }
                            })),
                    ),
            );
        }
        let content = if let Some(tab) = self
            .tabs
            .iter_mut()
            .find(|tab| Some(&tab.id) == self.selected.as_ref())
        {
            if let Some(error) = tab.error.clone().or_else(|| {
                tab.grid
                    .as_ref()
                    .and_then(|grid| grid.read(cx).launch_failure().map(str::to_string))
            }) {
                v_flex()
                    .p_3()
                    .gap_2()
                    .child(format!("Shell unavailable: {error}"))
                    .child(
                        Button::new("panel-restart")
                            .label("Restart shell")
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(tab) = this
                                    .tabs
                                    .iter_mut()
                                    .find(|tab| Some(&tab.id) == this.selected.as_ref())
                                {
                                    tab.grid = None;
                                    tab.error = None;
                                }
                                this.start_selected(cx);
                                cx.notify();
                            })),
                    )
                    .into_any_element()
            } else if let Some(grid) = &tab.grid {
                grid.clone().into_any_element()
            } else {
                div().p_3().child("Shell is starting.").into_any_element()
            }
        } else {
            div()
                .p_3()
                .child(if self.loading {
                    "Loading saved terminals…"
                } else {
                    "No terminal sessions. Click + to open one."
                })
                .into_any_element()
        };
        root.child(div().flex_1().min_h_0().min_w_0().child(content))
    }
}

#[cfg(test)]
mod tests;
