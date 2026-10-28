//! Category tree, filters, bookmark cards and the editor.
use buddy_core::bookmarks::{self, Bookmark, CategoryNode};
use gpui_kit::assets::IconName;
use gpui_kit::base::input::Paste;
use gpui_kit::base::{Disableable, Selectable};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, Textarea};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_kit::component::{ActiveTheme, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, ElementId, InteractiveElement, IntoElement,
    MouseButton, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Window,
    div,
};

use super::editor::Editor;
use super::{BookmarksView, CONTEXT, LoadState};
use crate::zoom::zpx;

/// The bookmark being dragged in the tree.
#[derive(Clone)]
pub(super) struct Dragged {
    pub id: String,
    title: SharedString,
}

struct DragPreview(SharedString);

impl Render for DragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().background)
            .child(self.0.clone())
    }
}

fn muted(text: impl Into<SharedString>, cx: &App) -> gpui_kit::Div {
    div()
        .whitespace_normal()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}

/// A field under its label; installed qualification finds fields by their labels.
fn labeled(label: impl Into<SharedString>, field: impl IntoElement) -> gpui_kit::Div {
    v_flex()
        .gap_1()
        .child(div().child(label.into()))
        .child(field)
}

fn category_label(name: &str) -> &str {
    if name.is_empty() {
        "Uncategorized"
    } else {
        name
    }
}

fn id(prefix: &str, key: &str) -> ElementId {
    ElementId::Name(format!("{prefix}-{key}").into())
}

/// The most room the tag filter takes above the list.
pub(super) const TAG_AREA_HEIGHT: f32 = 76.0;

impl BookmarksView {
    fn render_tree(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (border, primary) = (theme.border, theme.primary);
        let total = self.bookmarks().len();
        let mut tree = v_flex()
            .gap_1()
            .debug_selector(|| "bookmarks-tree".into())
            .child(
                h_flex()
                    .justify_between()
                    .child(div().text_sm().child("BOOKMARKS"))
                    .when(total > 0, |row| row.child(muted(total.to_string(), cx))),
            );
        match self.load_state() {
            LoadState::Loading => tree = tree.child(muted("Loading bookmarks…", cx)),
            LoadState::Failed(_) => tree = tree.child(muted("Unavailable", cx)),
            LoadState::Ready => {
                let all = self.filter.category.is_empty();
                tree = tree.child(
                    h_flex()
                        .id("bookmarks-all")
                        .gap_1()
                        .px_1()
                        .rounded_md()
                        .border_1()
                        .border_color(if all {
                            primary
                        } else {
                            gpui_kit::transparent_black()
                        })
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| this.select_category(None, cx)))
                        .child(Icon::new(IconName::Globe).size(zpx(14.)))
                        .child("All bookmarks"),
                );
                let data = self.data.as_ref();
                let categories = data
                    .map(|data| data.categories.as_slice())
                    .unwrap_or_default();
                let nodes = bookmarks::category_tree(categories, self.bookmarks());
                if nodes.is_empty() {
                    tree = tree.child(muted("No bookmarks yet", cx));
                }
                for node in &nodes {
                    tree = tree.child(self.render_node(node, 0, cx));
                }
            }
        }
        v_flex()
            .id("bookmarks-tree-pane")
            .w(zpx(260.))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .p_3()
            .border_r_1()
            .border_color(border)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| window.focus(&this.focus, cx)),
            )
            .child(tree)
            .into_any_element()
    }

    fn render_node(&self, node: &CategoryNode, depth: usize, cx: &mut Context<Self>) -> AnyElement {
        let primary = cx.theme().primary;
        let path = node.full_path.clone();
        let direct = bookmarks::in_order(self.bookmarks(), &node.full_path);
        let has_children = !node.children.is_empty() || !direct.is_empty();
        let expanded = self.expanded.contains(&node.full_path);
        let selected = self.filter.category == node.full_path;
        let label = category_label(&node.name).to_string();
        let indent = zpx(4. + depth as f32 * 14.);
        let toggle = path.clone();
        let select = path.clone();
        let row = h_flex()
            .id(id("bookmark-category", &path))
            .gap_1()
            .pl(indent)
            .pr_1()
            .rounded_md()
            .border_1()
            .border_color(if selected {
                primary
            } else {
                gpui_kit::transparent_black()
            })
            .child(if has_children {
                Button::new(id("bookmark-toggle", &path))
                    .ghost()
                    .xsmall()
                    .icon(if expanded {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    })
                    .tooltip(if expanded {
                        format!("Collapse {label}")
                    } else {
                        format!("Expand {label}")
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.toggle_expanded(toggle.clone(), cx)
                    }))
                    .into_any_element()
            } else {
                div().w(zpx(22.)).into_any_element()
            })
            .child(
                h_flex()
                    .id(id("bookmark-category-label", &path))
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select_category(Some(select.clone()), cx)
                    }))
                    .child(Icon::new(IconName::FolderOpen).size(zpx(14.)))
                    .child(div().flex_1().min_w_0().truncate().child(label))
                    .when(node.total_count > 0, |row| {
                        row.child(muted(node.total_count.to_string(), cx))
                    }),
            );
        let mut column = v_flex().child(row);
        if expanded {
            for child in &node.children {
                column = column.child(self.render_node(child, depth + 1, cx));
            }
            for bookmark in direct {
                column = column.child(self.render_tree_bookmark(bookmark, depth + 1, cx));
            }
        }
        column.into_any_element()
    }

    /// A bookmark under its category: click opens it, dragging it onto another bookmark
    /// of the same category reorders them, right-click offers the other actions.
    fn render_tree_bookmark(
        &self,
        bookmark: &Bookmark,
        depth: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let (accent, border) = (theme.accent, theme.border);
        let dragged = Dragged {
            id: bookmark.id.clone(),
            title: bookmark.title.clone().into(),
        };
        let view = cx.entity().downgrade();
        let (open, target) = (bookmark.id.clone(), bookmark.id.clone());
        let menu_id = bookmark.id.clone();
        h_flex()
            .id(id("bookmark-row", &bookmark.id))
            .gap_1()
            .pl(zpx(26. + depth as f32 * 14.))
            .pr_1()
            .rounded_md()
            .cursor_pointer()
            .hover(|style| style.bg(accent))
            .child(Icon::new(IconName::Globe).size(zpx(12.)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(bookmark.title.clone()),
            )
            .on_click(cx.listener(move |this, _, _, cx| this.open(&open, cx)))
            .on_drag(dragged, |dragged, _, _, cx| {
                cx.new(|_| DragPreview(dragged.title.clone()))
            })
            .drag_over::<Dragged>(move |style, _, _, _| style.border_t_2().border_color(border))
            .on_drop(cx.listener(move |this, dragged: &Dragged, _, cx| {
                this.drop_on(&dragged.id, &target, cx)
            }))
            .context_menu(move |menu, _, _| {
                let action = |label: &'static str,
                              run: fn(
                    &mut BookmarksView,
                    &str,
                    &mut Window,
                    &mut Context<BookmarksView>,
                )| {
                    let (view, id) = (view.clone(), menu_id.clone());
                    PopupMenuItem::new(label).on_click(move |_, window, cx| {
                        let _ = view.update(cx, |this, cx| run(this, &id, window, cx));
                    })
                };
                menu.item(action("Open", |this, id, _, cx| this.open(id, cx)))
                    .item(action("Open externally", |this, id, _, cx| {
                        this.open_externally(id, cx)
                    }))
                    .item(action("Edit", |this, id, window, cx| {
                        this.edit(id, window, cx)
                    }))
                    .separator()
                    .item(action("Move up", |this, id, _, cx| this.shift(id, -1, cx)))
                    .item(action("Move down", |this, id, _, cx| this.shift(id, 1, cx)))
                    .separator()
                    .item(action("Delete", |this, id, _, cx| this.ask_delete(id, cx)))
            })
            .into_any_element()
    }

    fn render_messages(&self, cx: &App) -> AnyElement {
        let mut messages = v_flex().gap_1();
        if let Some(notice) = &self.notice {
            messages = messages.child(
                div()
                    .whitespace_normal()
                    .debug_selector(|| "bookmarks-notice".into())
                    .child(notice.clone()),
            );
        }
        let refresh = self
            .data
            .is_some()
            .then_some(self.load_error.as_ref())
            .flatten()
            .map(|error| format!("Bookmarks could not be refreshed. {error}"));
        for error in self.error.iter().cloned().chain(refresh) {
            messages = messages.child(
                div()
                    .whitespace_normal()
                    .text_color(cx.theme().danger)
                    .debug_selector(|| "bookmarks-error".into())
                    .child(error),
            );
        }
        messages.into_any_element()
    }

    fn render_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let ready = self.load_state() == LoadState::Ready;
        let idle = ready && self.editor.is_none();
        v_flex()
            .gap_1()
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .items_center()
                    .child(div().flex_1().text_xl().child("Bookmarks"))
                    .child(
                        Button::new("bookmarks-paste")
                            .label("Paste link")
                            .icon(IconName::ExternalLink)
                            .disabled(!idle)
                            .on_click(cx.listener(|this, _, window, cx| this.paste(window, cx))),
                    )
                    .child(
                        Button::new("bookmarks-refresh")
                            .label(if self.loading {
                                "Refreshing…"
                            } else {
                                "Refresh"
                            })
                            .disabled(!ready || self.loading)
                            .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                    )
                    .child(
                        Button::new("bookmarks-add")
                            .primary()
                            .icon(IconName::Plus)
                            .label("Add")
                            .disabled(!idle)
                            .on_click(cx.listener(|this, _, window, cx| this.add(window, cx))),
                    ),
            )
            .when(idle, |column| {
                column.child(muted(
                    if cfg!(target_os = "macos") {
                        "Copy a link, then press Cmd+V here to save it as a bookmark."
                    } else {
                        "Copy a link, then press Ctrl+V here to save it as a bookmark."
                    },
                    cx,
                ))
            })
            .into_any_element()
    }

    fn render_filters(&self, cx: &mut Context<Self>) -> AnyElement {
        let tags = bookmarks::all_tags(self.bookmarks());
        // A large library has many tags; their own scrolling area keeps the list
        // usable at 900 x 650 (about three rows of chips).
        let mut chips = h_flex()
            .id("bookmarks-tags")
            .debug_selector(|| "bookmarks-tags".into())
            .flex_wrap()
            .gap_1()
            .items_center()
            .max_h(zpx(TAG_AREA_HEIGHT))
            .overflow_y_scroll();
        if !tags.is_empty() {
            chips = chips.child(
                Button::new("bookmarks-tag-all")
                    .xsmall()
                    .label("All tags")
                    .selected(self.filter.tag.is_empty())
                    .on_click(cx.listener(|this, _, _, cx| this.select_tag(None, cx))),
            );
        }
        for (index, tag) in tags.into_iter().enumerate() {
            let selected = self.filter.tag == tag;
            let chosen = tag.clone();
            chips = chips.child(
                Button::new(("bookmarks-tag", index))
                    .xsmall()
                    .label(format!("#{tag}"))
                    .selected(selected)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select_tag((!selected).then(|| chosen.clone()), cx)
                    })),
            );
        }
        let category = (!self.filter.category.is_empty())
            .then(|| muted(format!("In {}", self.filter.category), cx));
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .items_end()
                    .child(div().flex_1().child(labeled(
                        "Search",
                        Input::new(&self.search).prefix(Icon::new(IconName::Search)),
                    )))
                    .when(self.current_filter(cx).is_active(), |row| {
                        row.child(
                            Button::new("bookmarks-clear-filters")
                                .label("Clear filters")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.clear_filters(window, cx)
                                })),
                        )
                    }),
            )
            .children(category)
            .child(chips)
            .into_any_element()
    }

    pub(super) fn render_card(
        &self,
        index: usize,
        bookmark: &Bookmark,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let (border, danger) = (theme.border, theme.danger);
        let confirming = self.confirm.as_deref() == Some(bookmark.id.as_str());
        let (open, open_tab, external, edit, delete) = (
            bookmark.id.clone(),
            bookmark.id.clone(),
            bookmark.id.clone(),
            bookmark.id.clone(),
            bookmark.id.clone(),
        );
        let mut meta = h_flex()
            .flex_wrap()
            .gap_2()
            .child(muted(bookmark.category_leaf().to_string(), cx));
        for tag in &bookmark.tags {
            meta = meta.child(muted(format!("#{tag}"), cx));
        }
        let mut card = v_flex()
            .id(("bookmark-card", index))
            .gap_1()
            .p_2()
            .rounded_md()
            .border_1()
            .border_color(if confirming { danger } else { border })
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .items_start()
                    .child(
                        v_flex()
                            .id(("bookmark-open", index))
                            .flex_1()
                            .min_w(zpx(200.))
                            .gap_1()
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| this.open(&open, cx)))
                            .child(div().text_lg().child(bookmark.title.clone()))
                            .child(muted(bookmark.url.clone(), cx))
                            .when_some(
                                bookmark.description.clone().filter(|d| !d.is_empty()),
                                |column, description| {
                                    column.child(div().whitespace_normal().child(description))
                                },
                            )
                            .child(meta),
                    )
                    .child(
                        h_flex()
                            .flex_wrap()
                            .gap_1()
                            // The card opens in a Buddy tab too; this says so.
                            .child(
                                Button::new(("bookmark-open-tab", index))
                                    .small()
                                    .icon(IconName::Globe)
                                    .label("Open in tab")
                                    .on_click(
                                        cx.listener(move |this, _, _, cx| this.open(&open_tab, cx)),
                                    ),
                            )
                            .child(
                                Button::new(("bookmark-external", index))
                                    .small()
                                    .icon(IconName::ExternalLink)
                                    .label("Open externally")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.open_externally(&external, cx)
                                    })),
                            )
                            .child(
                                Button::new(("bookmark-edit", index))
                                    .small()
                                    .label("Edit")
                                    .disabled(self.editor.is_some())
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.edit(&edit, window, cx)
                                    })),
                            )
                            .child(
                                Button::new(("bookmark-delete", index))
                                    .small()
                                    .label("Delete")
                                    .disabled(self.busy || confirming)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.ask_delete(&delete, cx)
                                    })),
                            ),
                    ),
            );
        if confirming {
            card = card.child(self.render_confirm(bookmark, cx));
        }
        card.into_any_element()
    }

    /// Electron's ConfirmDialog text, inline under the card, as the app's other
    /// destructive actions ask.
    fn render_confirm(&self, bookmark: &Bookmark, cx: &mut Context<Self>) -> AnyElement {
        let danger = cx.theme().danger;
        v_flex()
            .gap_1()
            .pt_1()
            .debug_selector(|| "bookmarks-confirm".into())
            .child(div().child(format!("Delete \u{201c}{}\u{201d}?", bookmark.title)))
            .child(match &self.delete_error {
                Some(error) => div()
                    .whitespace_normal()
                    .text_color(danger)
                    .child(error.clone()),
                None => muted("This bookmark will be permanently removed.", cx),
            })
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("bookmark-confirm-delete")
                            .danger()
                            .label(if self.busy {
                                "Deleting…"
                            } else {
                                "Confirm delete"
                            })
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, _, cx| this.confirm_delete(cx))),
                    )
                    .child(
                        Button::new("bookmark-cancel-delete")
                            .label("Cancel")
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, _, cx| this.cancel_delete(cx))),
                    ),
            )
            .into_any_element()
    }

    fn render_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let filter = self.current_filter(cx);
        let mut shown = filter.apply(self.bookmarks());
        // A delete asked from the tree stays answerable while filters hide its card.
        if let Some(target) = self.confirm.as_deref().and_then(|id| self.find(id))
            && !shown.iter().any(|bookmark| bookmark.id == target.id)
        {
            shown.insert(0, target);
        }
        let mut list = v_flex()
            .id("bookmarks-list")
            .flex_1()
            .min_h_0()
            .gap_2()
            .overflow_y_scroll()
            .debug_selector(|| "bookmarks-list".into())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| window.focus(&this.focus, cx)),
            );
        if shown.is_empty() {
            list = list.child(if self.bookmarks().is_empty() {
                v_flex()
                    .gap_1()
                    .py_4()
                    .items_center()
                    .child(Icon::new(IconName::Globe).size(zpx(40.)))
                    .child(div().child("No bookmarks yet"))
                    .child(muted(
                        "Choose Add, or paste a URL here to save your first link.",
                        cx,
                    ))
                    .into_any_element()
            } else {
                v_flex()
                    .gap_1()
                    .py_4()
                    .items_center()
                    .child(Icon::new(IconName::Search).size(zpx(40.)))
                    .child(div().child("No bookmarks match your filters"))
                    .into_any_element()
            });
        }
        let cards: Vec<AnyElement> = shown
            .iter()
            .enumerate()
            .map(|(index, bookmark)| self.render_card(index, bookmark, cx))
            .collect();
        list.children(cards).into_any_element()
    }

    fn render_editor(&self, editor: &Editor, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (border, danger) = (theme.border, theme.danger);
        let categories: Vec<String> = self
            .data
            .as_ref()
            .map(|data| {
                let mut all: Vec<String> = data
                    .categories
                    .iter()
                    .cloned()
                    .chain(
                        data.bookmarks
                            .iter()
                            .map(|bookmark| bookmark.category.clone()),
                    )
                    .filter(|category| !category.is_empty())
                    .collect();
                all.sort();
                all.dedup();
                all
            })
            .unwrap_or_default();
        let mut existing = h_flex().flex_wrap().gap_1();
        for (index, category) in categories.into_iter().enumerate() {
            let chosen = category.clone();
            existing = existing.child(
                Button::new(("bookmark-use-category", index))
                    .xsmall()
                    .label(category)
                    .disabled(editor.saving)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.use_category(chosen.clone(), window, cx)
                    })),
            );
        }
        let title_label = if editor.fetching_title {
            "Title (fetching…)"
        } else {
            "Title"
        };
        let (description_label, tags_label) = if editor.suggesting {
            (
                "Description (Copilot is suggesting…)",
                "Tags (Copilot is suggesting…)",
            )
        } else {
            ("Description", "Tags")
        };
        let saving = editor.saving;
        let actions = h_flex()
            .gap_2()
            .child(
                Button::new("bookmark-editor-save")
                    .primary()
                    .label(if saving {
                        "Saving…"
                    } else if editor.editing.is_some() {
                        "Save Changes"
                    } else {
                        "Add Bookmark"
                    })
                    .disabled(saving)
                    .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
            )
            .child(
                Button::new("bookmark-editor-cancel")
                    .label("Cancel")
                    .disabled(saving)
                    .on_click(cx.listener(|this, _, window, cx| this.close_editor(window, cx))),
            );
        // Two fields per row, and the actions in the heading, so the whole editor fits
        // the compact window; rows wrap when the pane is narrow.
        let pair = |left: gpui_kit::Div, right: gpui_kit::Div| {
            h_flex()
                .flex_wrap()
                .gap_3()
                .child(left.flex_1().min_w(zpx(220.)))
                .child(right.flex_1().min_w(zpx(220.)))
        };
        v_flex()
            .id("bookmarks-editor")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .gap_2()
            .p_3()
            .border_1()
            .rounded_md()
            .border_color(border)
            .debug_selector(|| "bookmarks-editor".into())
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .items_center()
                    .child(div().flex_1().text_lg().child(if editor.editing.is_some() {
                        "Edit bookmark"
                    } else {
                        "New bookmark"
                    }))
                    .child(actions),
            )
            .when_some(editor.error.clone(), |column, error| {
                column.child(
                    div()
                        .whitespace_normal()
                        .text_color(danger)
                        .debug_selector(|| "bookmarks-editor-error".into())
                        .child(error),
                )
            })
            .child(pair(
                labeled("URL", Input::new(&editor.url).disabled(saving)),
                labeled(title_label, Input::new(&editor.title).disabled(saving)),
            ))
            .child(pair(
                labeled("Category", Input::new(&editor.category).disabled(saving)),
                labeled(tags_label, Input::new(&editor.tags).disabled(saving)),
            ))
            .child(muted(
                "URL, title and category are required. Categories nest with / (e.g. \
                 Development/Frontend): pick an existing one below, then type /Name after it for \
                 a new subcategory. Tags are comma-separated, at most 50.",
                cx,
            ))
            .child(existing)
            .child(
                labeled(
                    description_label,
                    Textarea::new(&editor.description)
                        .disabled(saving)
                        .h(zpx(64.)),
                )
                .debug_selector(|| "bookmarks-editor-description".into()),
            )
            .into_any_element()
    }

    fn render_failure(&self, error: String, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .gap_2()
            .p_4()
            .items_start()
            .debug_selector(|| "bookmarks-failure".into())
            .child(div().text_lg().child("Unable to load bookmarks"))
            .child(
                div()
                    .whitespace_normal()
                    .text_color(cx.theme().danger)
                    .child(error),
            )
            .child(
                Button::new("bookmarks-retry")
                    .label("Retry")
                    .on_click(cx.listener(|this, _, _, cx| this.retry(cx))),
            )
            .into_any_element()
    }
}

impl Render for BookmarksView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.focus_next) && self.editor.is_none() {
            window.focus(&self.focus, cx);
        }
        let main = v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .gap_3()
            .p_4()
            .child(self.render_header(cx))
            .child(self.render_messages(cx));
        let main = match self.load_state() {
            LoadState::Loading => main.child(muted("Loading bookmarks…", cx)),
            LoadState::Failed(error) => main.child(self.render_failure(error, cx)),
            LoadState::Ready => {
                let body = match &self.editor {
                    Some(editor) => self.render_editor(editor, cx),
                    None => self.render_list(cx),
                };
                main.when(self.editor.is_none(), |main| {
                    main.child(self.render_filters(cx))
                })
                .child(body)
            }
        };
        h_flex()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &Paste, window, cx| this.paste(window, cx)))
            .size_full()
            .child(self.render_tree(cx))
            .child(main)
    }
}
