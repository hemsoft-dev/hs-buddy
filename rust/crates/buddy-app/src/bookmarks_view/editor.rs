//! The Add/Edit bookmark editor. A field differs from what Buddy last wrote into it
//! only when the user typed there, so fetched titles and AI suggestions never replace
//! the user's own text.
use buddy_core::bookmarks::Form;
use gpui_kit::component::input::{InputEvent, InputState, TextareaState};
use gpui_kit::{AppContext as _, Context, Entity, Window};

use super::BookmarksView;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Field {
    Title,
    Description,
    Tags,
}

pub(super) struct Editor {
    /// The bookmark being edited; `None` adds one.
    pub editing: Option<String>,
    pub url: Entity<InputState>,
    pub title: Entity<InputState>,
    pub description: Entity<TextareaState>,
    pub category: Entity<InputState>,
    pub tags: Entity<InputState>,
    /// What Buddy itself last put in each field.
    written: Form,
    /// The URL that was pasted to open this editor; its title and suggestion are
    /// only applied while the URL field still holds it.
    pub pasted: Option<String>,
    pub fetching_title: bool,
    pub suggesting: bool,
    pub saving: bool,
    pub error: Option<String>,
    /// Distinguishes this editor's background work from an earlier editor's.
    pub request: u64,
}

impl Editor {
    pub fn new(
        form: Form,
        editing: Option<String>,
        pasted: Option<String>,
        request: u64,
        window: &mut Window,
        cx: &mut Context<BookmarksView>,
    ) -> Self {
        let input = |text: &str,
                     placeholder: &'static str,
                     window: &mut Window,
                     cx: &mut Context<BookmarksView>| {
            let text = text.to_string();
            let state = cx.new(|cx| {
                let mut state = InputState::new(window, cx).placeholder(placeholder);
                state.set_value(text, window, cx);
                state
            });
            cx.subscribe_in(&state, window, |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.save(window, cx);
                } else {
                    cx.notify();
                }
            })
            .detach();
            state
        };
        let url = input(&form.url, "https://example.com", window, cx);
        let title = input(&form.title, "My Bookmark", window, cx);
        let category = input(&form.category, "Category/Subcategory", window, cx);
        let tags = input(&form.tags, "tag1, tag2, tag3", window, cx);
        let text = form.description.clone();
        let description = cx.new(|cx| {
            let mut state = TextareaState::new(window, cx).placeholder("Optional description…");
            state.set_value(text, window, cx);
            state
        });
        cx.subscribe(&description, |_, _, _: &InputEvent, cx| cx.notify())
            .detach();
        Self {
            editing,
            url,
            title,
            description,
            category,
            tags,
            written: form,
            pasted,
            fetching_title: false,
            suggesting: false,
            saving: false,
            error: None,
            request,
        }
    }

    pub fn form(&self, cx: &gpui_kit::App) -> Form {
        Form {
            url: self.url.read(cx).value().to_string(),
            title: self.title.read(cx).value().to_string(),
            description: self.description.read(cx).value().to_string(),
            category: self.category.read(cx).value().to_string(),
            tags: self.tags.read(cx).value().to_string(),
        }
    }

    pub fn edited(&self, field: Field, cx: &gpui_kit::App) -> bool {
        let form = self.form(cx);
        match field {
            Field::Title => form.title != self.written.title,
            Field::Description => form.description != self.written.description,
            Field::Tags => form.tags != self.written.tags,
        }
    }

    /// Whether the URL field still holds the pasted URL.
    pub fn holds_pasted(&self, cx: &gpui_kit::App) -> bool {
        self.pasted
            .as_deref()
            .is_some_and(|pasted| self.url.read(cx).value().trim() == pasted)
    }

    /// Fill a field the user has not edited.
    pub fn offer(
        &mut self,
        field: Field,
        text: String,
        window: &mut Window,
        cx: &mut gpui_kit::App,
    ) {
        if self.edited(field, cx) {
            return;
        }
        match field {
            Field::Title => {
                self.written.title = text.clone();
                self.title
                    .update(cx, |input, cx| input.set_value(text, window, cx));
            }
            Field::Description => {
                self.written.description = text.clone();
                self.description
                    .update(cx, |input, cx| input.set_value(text, window, cx));
            }
            Field::Tags => {
                self.written.tags = text.clone();
                self.tags
                    .update(cx, |input, cx| input.set_value(text, window, cx));
            }
        }
    }

    /// Put an existing category into the category field, as a starting point for a
    /// new subcategory too.
    pub fn use_category(&mut self, category: &str, window: &mut Window, cx: &mut gpui_kit::App) {
        let text = category.to_string();
        self.category
            .update(cx, |input, cx| input.set_value(text, window, cx));
    }
}
