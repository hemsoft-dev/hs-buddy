//! Folder explorer: lazy tree and read-only, highlighted source preview.
//!
//! File > Open Folder (Ctrl/Cmd+O outside terminal input), or a pane's Folder View,
//! opens a root. Close returns to the preceding section. The tree caches each
//! expanded directory for this opening, matches Electron's hidden/generated
//! entry policy, and shows read failures inline. Preview reads are capped at
//! 1 MiB even if a file grows; each root/selection has a request identity so a
//! late read cannot replace the current file. Source stays read-only and local.
//! Syntect supplies syntax colors, with plain text for unknown languages.
//! Global application tabs and persistent navigation belong to the separate
//! shell parity work in #823.
mod chooser;
mod files;
#[cfg(test)]
mod tests;

use crate::zoom::{scaled, zpx};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Editor, EditorState, InputState};
use gpui_kit::component::resizable::{h_resizable, resizable_panel};
use gpui_kit::component::{ActiveTheme, Icon, Selectable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    AppContext as _, Context, Entity, EventEmitter, InteractiveElement, IntoElement, ParentElement,
    Render, StatefulInteractiveElement, Styled, Window, div,
};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

gpui_kit::actions!(explorer, [OpenFolder]);

pub fn init(cx: &mut gpui_kit::App) {
    // Terminal control keys belong to the shell. Negated context predicates
    // inspect the whole focus path, including ancestors of focused children.
    cx.bind_keys([gpui_kit::KeyBinding::new(
        "secondary-o",
        OpenFolder,
        Some("!BuddyTerminal && !BuddyTerminalPaste"),
    )]);
}

pub struct Close;
pub struct OpenPath(pub PathBuf);
impl EventEmitter<OpenPath> for crate::terminal_view::TerminalView {}
pub struct ExplorerView {
    root: PathBuf,
    generation: u64,
    children: HashMap<PathBuf, Vec<files::Entry>>,
    expanded: HashSet<PathBuf>,
    pending: HashSet<PathBuf>,
    errors: HashMap<PathBuf, String>,
    selected: Option<PathBuf>,
    preview_request: u64,
    preview: Option<Result<files::Preview, String>>,
    editor: Option<Entity<EditorState>>,
    chooser: Option<Entity<InputState>>,
    chooser_error: Option<String>,
    focus: Option<gpui_kit::FocusHandle>,
}
impl EventEmitter<Close> for ExplorerView {}
impl ExplorerView {
    pub fn new() -> Self {
        Self {
            root: PathBuf::new(),
            generation: 0,
            children: HashMap::new(),
            expanded: HashSet::new(),
            pending: HashSet::new(),
            errors: HashMap::new(),
            selected: None,
            preview_request: 0,
            preview: None,
            editor: None,
            chooser: None,
            chooser_error: None,
            focus: None,
        }
    }
    pub fn open(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.chooser = None;
        self.chooser_error = None;
        self.generation += 1;
        self.root = path.clone();
        self.children.clear();
        self.expanded.clear();
        self.pending.clear();
        self.errors.clear();
        self.selected = None;
        self.preview = None;
        self.editor = None;
        self.load(path, cx);
    }
    fn load(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if !self.pending.insert(path.clone()) {
            return;
        }
        self.errors.remove(&path);
        let generation = self.generation;
        let read_path = path.clone();
        let task = cx
            .background_executor()
            .spawn(async move { files::directory(&read_path) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.pending.remove(&path);
                match result {
                    Ok(entries) => {
                        this.children.insert(path, entries);
                    }
                    Err(error) => {
                        this.errors.insert(path, error);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn toggle(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if !self.expanded.remove(&path) {
            self.expanded.insert(path.clone());
            if !self.children.contains_key(&path) {
                self.load(path, cx);
            }
        }
        cx.notify();
    }
    fn select(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = Some(path.clone());
        self.preview = None;
        self.editor = None;
        self.preview_request += 1;
        let request = (self.generation, self.preview_request);
        let task = cx
            .background_executor()
            .spawn(async move { files::preview(&path) });
        let handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = handle.update(cx, |_, window, cx| {
                this.update(cx, |this, cx| {
                    if request != (this.generation, this.preview_request) {
                        return;
                    }
                    if let Ok(data) = &result {
                        this.editor = Some(cx.new(|cx| {
                            EditorState::new(window, cx)
                                .language(data.language)
                                .soft_wrap(false)
                                .line_number(true)
                                .default_value(data.content.clone())
                        }));
                    }
                    this.preview = Some(result);
                    cx.notify();
                })
            });
        })
        .detach();
        cx.notify();
    }
    fn rows(&self, path: &Path, depth: usize, out: &mut Vec<(files::Entry, usize)>) {
        if let Some(entries) = self.children.get(path) {
            for entry in entries {
                out.push((entry.clone(), depth));
                if self.expanded.contains(&entry.path) {
                    self.rows(&entry.path, depth + 1, out);
                }
            }
        }
    }
}
impl Render for ExplorerView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focus = self.focus.get_or_insert_with(|| cx.focus_handle()).clone();
        if self.chooser.is_some() {
            return v_flex()
                .id("folder-explorer")
                .role(gpui_kit::Role::Group)
                .size_full()
                .track_focus(&focus)
                .child(self.chooser_form(cx));
        }
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let mut rows = Vec::new();
        self.rows(&self.root, 0, &mut rows);
        let mut tree = v_flex()
            .id("folder-tree")
            .size_full()
            .overflow_y_scroll()
            .text_size(zpx(12.));
        if self.pending.contains(&self.root) {
            tree = tree.child("Loading...");
        }
        if self.children.get(&self.root).is_some_and(Vec::is_empty) {
            tree = tree.child("Empty directory");
        }
        for (entry, depth) in rows {
            let path = entry.path.clone();
            let directory = entry.directory;
            let expanded = self.expanded.contains(&path);
            let active = self.selected.as_ref() == Some(&path);
            let icon = if directory {
                if expanded {
                    IconName::FolderOpen
                } else {
                    IconName::Folder
                }
            } else {
                IconName::File
            };
            let row = Button::new(path.to_string_lossy().into_owned())
                .ghost()
                .small()
                .w_full()
                .justify_start()
                .rounded_none()
                .pl(zpx(8. + depth as f32 * 16.))
                .selected(active)
                .child(
                    h_flex()
                        .w_full()
                        .gap(zpx(4.))
                        .justify_start()
                        .child(div().w(zpx(12.)).child(if directory {
                            if expanded { "⌄" } else { "›" }
                        } else {
                            ""
                        }))
                        .child(Icon::new(icon).size(zpx(14.)))
                        .child(div().min_w_0().truncate().child(entry.name.clone())),
                )
                .accessibility_label(entry.path.to_string_lossy().into_owned())
                .tooltip(entry.path.to_string_lossy().into_owned())
                .on_click(cx.listener(move |this, _, window, cx| {
                    if directory {
                        this.toggle(path.clone(), cx);
                    } else {
                        this.select(path.clone(), window, cx);
                    }
                }));
            tree = tree.child(row);
            if expanded && self.pending.contains(&entry.path) {
                tree = tree.child(div().pl(zpx(24. + depth as f32 * 16.)).child("Loading..."));
            }
            if expanded && self.children.get(&entry.path).is_some_and(Vec::is_empty) {
                tree = tree.child(
                    div()
                        .pl(zpx(24. + depth as f32 * 16.))
                        .text_color(muted)
                        .child("Empty directory"),
                );
            }
            if expanded && let Some(error) = self.errors.get(&entry.path) {
                tree = tree.child(div().child(error.clone()));
            }
        }
        if let Some(error) = self.errors.get(&self.root) {
            tree = tree.child(error.clone());
        }
        let header = self
            .selected
            .as_ref()
            .map(|p| {
                p.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            })
            .unwrap_or_default();
        let mut preview = v_flex()
            .size_full()
            .min_w_0()
            .min_h_0()
            .child(div().px(zpx(12.)).py(zpx(8.)).child(header));
        match &self.preview {
            Some(Ok(data)) => {
                let lines = if data.content.is_empty() {
                    0
                } else {
                    data.content.split('\n').count()
                };
                preview = preview.child(div().px(zpx(12.)).text_color(muted).child(format!(
                    "{} · {lines} {} · {}",
                    data.language,
                    if lines == 1 { "line" } else { "lines" },
                    files::size(data.size)
                )));
                if let Some(editor) = &self.editor {
                    preview = preview.child(
                        div().flex_1().min_h_0().child(
                            Editor::new(editor)
                                .aria_label("Read-only file preview")
                                .role(gpui_kit::Role::MultilineTextInput)
                                .readonly(true)
                                .h(gpui_kit::relative(1.))
                                .bordered(false),
                        ),
                    );
                }
            }
            Some(Err(error)) => preview = preview.child(div().p(zpx(16.)).child(error.clone())),
            None => {
                preview = preview.child(div().p(zpx(24.)).text_color(muted).child(
                    if self.selected.is_some() {
                        "Loading..."
                    } else {
                        "Select a file to preview"
                    },
                ))
            }
        }
        v_flex()
            .id("folder-explorer")
            .role(gpui_kit::Role::Group)
            .track_focus(&focus)
            .size_full()
            .min_h_0()
            .child(
                h_flex()
                    .gap(zpx(8.))
                    .px(zpx(8.))
                    .py(zpx(5.))
                    .child(Icon::new(IconName::FolderOpen).size(zpx(14.)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(self.root.display().to_string()),
                    )
                    .child(
                        Button::new("close-folder")
                            .ghost()
                            .label("Close folder")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(Close))),
                    ),
            )
            .child(
                div().flex_1().min_h_0().child(
                    h_resizable("folder-split")
                        .child(
                            resizable_panel()
                                .size(scaled(cx, 240.))
                                .size_range(scaled(cx, 180.)..scaled(cx, 500.))
                                .child(tree),
                        )
                        .child(
                            resizable_panel()
                                .size_range(scaled(cx, 300.)..scaled(cx, 10000.))
                                .child(preview),
                        ),
                ),
            )
    }
}
