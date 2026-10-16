mod ime;
mod paint;

use std::ops::Range;
use std::sync::Arc;
use std::time::{Duration, Instant};

use buddy_core::terminal::{
    InputError, InputStatus, InputTicket, Key, Modifiers, ScreenSnapshot, Session, SessionStatus,
};
use gpui_kit::component::button::Button;
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::{
    ClipboardItem, Context, EventEmitter, FocusHandle, InteractiveElement, IntoElement,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement, Pixels, Point,
    Render, Role, ScrollDelta, ScrollWheelEvent, StatefulInteractiveElement, Styled, Window, div,
};

use crate::zoom::zpx;

pub enum GridEvent {
    FocusControls,
    WorkingDirectory(String),
    Selected,
    PasteWritten { tag: Option<String> },
    PasteUncertain { tag: Option<String> },
    PasteNotSent { tag: Option<String> },
}

gpui_kit::actions!(terminal, [ShellTab, ShellBackTab, FocusControls]);

pub(super) fn init(cx: &mut gpui_kit::App) {
    cx.bind_keys([
        gpui_kit::KeyBinding::new("tab", ShellTab, Some("BuddyTerminal")),
        gpui_kit::KeyBinding::new("shift-tab", ShellBackTab, Some("BuddyTerminal")),
        gpui_kit::KeyBinding::new("ctrl-shift-p", FocusControls, Some("BuddyTerminal")),
    ]);
}

fn native_text(text: Option<&str>, control: bool, alt: bool, macos: bool) -> bool {
    text.is_some_and(|text| (!control || alt) && (!alt || macos || control || !text.is_ascii()))
}

struct Paste {
    text: String,
    tag: Option<String>,
    copy: bool,
}

struct PendingPaste {
    paste: Paste,
    ticket: InputTicket,
    started: Instant,
}

pub struct TerminalGrid {
    session: Arc<Session>,
    screen: Arc<ScreenSnapshot>,
    revision: u64,
    focus: FocusHandle,
    visible: bool,
    selected: bool,
    bounds: Option<gpui_kit::Bounds<Pixels>>,
    cell_size: gpui_kit::Size<Pixels>,
    selection: Option<(usize, usize)>,
    dragging: bool,
    scroll_fraction: f32,
    composition: String,
    composition_selection: Range<usize>,
    confirmation: Option<Paste>,
    pending: Option<PendingPaste>,
    unsent_text: Option<String>,
    status: Option<String>,
    session_status: SessionStatus,
}

impl EventEmitter<GridEvent> for TerminalGrid {}

impl TerminalGrid {
    pub fn new(session: Arc<Session>, cx: &mut Context<Self>) -> Self {
        let snapshot = session.snapshot();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(33))
                    .await;
                if this.update(cx, |this, cx| this.poll(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        Self {
            session,
            screen: Arc::new(snapshot.screen),
            revision: snapshot.revision,
            focus: cx.focus_handle(),
            visible: false,
            selected: false,
            bounds: None,
            cell_size: gpui_kit::size(gpui_kit::px(8.0), gpui_kit::px(18.0)),
            selection: None,
            dragging: false,
            scroll_fraction: 0.0,
            composition: String::new(),
            composition_selection: 0..0,
            confirmation: None,
            pending: None,
            unsent_text: None,
            status: None,
            session_status: snapshot.status,
        }
    }

    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
    }

    pub fn set_visibility(&mut self, visible: bool, selected: bool, cx: &mut Context<Self>) {
        if !visible || !selected {
            if let Some(paste) = self.confirmation.take() {
                cx.emit(GridEvent::PasteNotSent { tag: paste.tag });
            }
            self.dragging = false;
            if !self.composition.is_empty() {
                self.unsent_text = Some(std::mem::take(&mut self.composition));
                self.status = Some("Composition retained as unsent input.".into());
            }
            self.composition_selection = 0..0;
        }
        self.visible = visible;
        self.selected = selected;
        self.poll(cx);
    }

    fn poll(&mut self, cx: &mut Context<Self>) {
        let revision = self.session.revision();
        if self.visible && self.revision != revision {
            let snapshot = self.session.snapshot();
            if snapshot.screen.working_directory != self.screen.working_directory
                && let Some(cwd) = snapshot
                    .screen
                    .working_directory
                    .as_ref()
                    .and_then(|path| path.to_str())
            {
                cx.emit(GridEvent::WorkingDirectory(cwd.into()));
            }
            if self.screen.size != snapshot.screen.size
                || self.screen.scroll_offset != snapshot.screen.scroll_offset
            {
                self.selection = None;
            }
            self.screen = Arc::new(snapshot.screen);
            self.session_status = snapshot.status;
            self.revision = snapshot.revision;
            cx.notify();
        }
        let Some(pending) = self.pending.as_ref() else {
            return;
        };
        match self.session.input_status(&pending.ticket) {
            Some(InputStatus::Written) => {
                let pending = self.pending.take().expect("checked pending paste");
                if pending.paste.copy && self.visible && self.selected {
                    cx.write_to_clipboard(ClipboardItem::new_string(pending.paste.text));
                }
                self.status = Some("Paste written to the terminal. No Enter key was added.".into());
                cx.emit(GridEvent::PasteWritten {
                    tag: pending.paste.tag,
                });
                cx.notify();
            }
            Some(InputStatus::Pending) if pending.started.elapsed() < Duration::from_secs(10) => {}
            _ => {
                let pending = self.pending.take().expect("checked pending paste");
                self.status = Some(
                    "Paste delivery is uncertain. It will not be sent again automatically.".into(),
                );
                cx.emit(GridEvent::PasteUncertain {
                    tag: pending.paste.tag,
                });
                cx.notify();
            }
        }
    }

    /// A tag identifies an auxiliary prompt action, never a command to replay.
    pub fn paste(&mut self, text: String, tag: Option<String>, copy: bool, cx: &mut Context<Self>) {
        if !self.visible || !self.selected || self.pending.is_some() || self.confirmation.is_some()
        {
            self.status = Some("Finish the current paste before starting another.".into());
            cx.emit(GridEvent::PasteNotSent { tag });
            cx.notify();
            return;
        }
        self.send_paste(Paste { text, tag, copy }, false, cx);
    }

    fn send_paste(&mut self, paste: Paste, confirmed: bool, cx: &mut Context<Self>) {
        match self.session.paste(&paste.text, confirmed) {
            Ok(ticket) => {
                self.pending = Some(PendingPaste {
                    paste,
                    ticket,
                    started: Instant::now(),
                });
                self.status = Some("Waiting for the terminal write acknowledgement…".into());
                self.selection = None;
            }
            Err(InputError::ConfirmMultiline) => self.confirmation = Some(paste),
            Err(error) => {
                self.unsent_text = Some(paste.text);
                self.status = Some(error.to_string());
                cx.emit(GridEvent::PasteNotSent { tag: paste.tag });
            }
        }
        cx.notify();
    }

    fn selection_range(&self) -> Option<Range<usize>> {
        let (a, b) = self.selection?;
        (a != b).then_some(a.min(b)..a.max(b).saturating_add(1))
    }

    fn copy(&self, cx: &mut Context<Self>) {
        if let Some(range) = self.selection_range() {
            cx.write_to_clipboard(ClipboardItem::new_string(self.screen.selected_text(range)));
        }
    }

    fn position(&self, position: Point<Pixels>) -> usize {
        let Some(bounds) = self.bounds else {
            return 0;
        };
        let columns = usize::from(self.screen.size.columns());
        let rows = usize::from(self.screen.size.rows());
        let column = ((position.x - bounds.left()) / self.cell_size.width)
            .floor()
            .max(0.0) as usize;
        let row = ((position.y - bounds.top()) / self.cell_size.height)
            .floor()
            .max(0.0) as usize;
        row.min(rows - 1) * columns + column.min(columns - 1)
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.visible || !self.selected {
            return;
        }
        let stroke = &event.keystroke;
        let modifiers = stroke.modifiers;
        let clipboard = if cfg!(target_os = "macos") {
            modifiers.platform
        } else {
            modifiers.control && modifiers.shift
        };
        if clipboard && stroke.key == "c" {
            self.copy(cx);
            cx.stop_propagation();
            return;
        }
        if clipboard && stroke.key == "v" {
            if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                self.paste(text, None, false, cx);
            }
            cx.stop_propagation();
            return;
        }
        if !self.composition.is_empty() || modifiers.platform {
            return;
        }
        let key = match stroke.key.as_str() {
            "enter" | "return" => Key::Enter,
            "backspace" => Key::Backspace,
            "tab" => Key::Tab,
            "escape" => Key::Escape,
            "up" => Key::Up,
            "down" => Key::Down,
            "left" => Key::Left,
            "right" => Key::Right,
            "home" => Key::Home,
            "end" => Key::End,
            "insert" => Key::Insert,
            "delete" => Key::Delete,
            "pageup" => Key::PageUp,
            "pagedown" => Key::PageDown,
            "space" if modifiers.control => Key::Text(" "),
            key if key
                .strip_prefix('f')
                .and_then(|n| n.parse::<u8>().ok())
                .is_some_and(|n| (1..=12).contains(&n)) =>
            {
                Key::Function(stroke.key[1..].parse().expect("checked function key"))
            }
            // Native printable text, including AltGr and Option characters,
            // enters once through EntityInputHandler after IME commit.
            _ if native_text(
                stroke.key_char.as_deref(),
                modifiers.control,
                modifiers.alt,
                cfg!(target_os = "macos"),
            ) =>
            {
                return;
            }
            _ if modifiers.alt && !modifiers.control => {
                Key::Text(stroke.key_char.as_deref().unwrap_or(&stroke.key))
            }
            _ if modifiers.control => Key::Text(&stroke.key),
            _ => return,
        };
        if let Err(error) = self.session.key(
            key,
            Modifiers {
                control: modifiers.control,
                alt: modifiers.alt,
                shift: modifiers.shift,
            },
        ) {
            self.status = Some(error.to_string());
            window.play_system_bell();
        }
        self.selection = None;
        cx.stop_propagation();
        cx.notify();
    }
}

impl Render for TerminalGrid {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut root = v_flex().size_full().min_h_0();
        if let Some(paste) = &self.confirmation {
            root = root.child(v_flex().gap_2().p_2().border_1().border_color(cx.theme().border)
                .child(format!("Paste {} lines? This shell does not bracket pastes, so line breaks may execute commands immediately.", paste.text.lines().count()))
                .child(h_flex().gap_2()
                    .child(Button::new("confirm-terminal-paste").label("Paste these lines").on_click(cx.listener(|this, _, _, cx| {
                        if this.visible && this.selected && let Some(paste) = this.confirmation.take() {
                            this.send_paste(paste, true, cx);
                        }
                    })))
                    .child(Button::new("cancel-terminal-paste").label("Cancel").on_click(cx.listener(|this, _, _, cx| {
                        if let Some(paste)=this.confirmation.take(){cx.emit(GridEvent::PasteNotSent{tag:paste.tag});} cx.notify();
                    })))));
        }
        let body = div()
            .id("terminal-grid")
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_hidden()
            .role(Role::Terminal)
            .aria_label("Interactive terminal")
            .aria_description(self.screen.text())
            .track_focus(&self.focus)
            .key_context("BuddyTerminal")
            .on_action(cx.listener(|this, _: &ShellTab, _, cx| {
                if this.visible
                    && this.selected
                    && this.composition.is_empty()
                    && let Err(error) = this.session.key(Key::Tab, Modifiers::default())
                {
                    this.status = Some(error.to_string());
                }
                cx.stop_propagation();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ShellBackTab, _, cx| {
                if this.visible
                    && this.selected
                    && this.composition.is_empty()
                    && let Err(error) = this.session.key(
                        Key::Tab,
                        Modifiers {
                            shift: true,
                            ..Default::default()
                        },
                    )
                {
                    this.status = Some(error.to_string());
                }
                cx.stop_propagation();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &FocusControls, _, cx| {
                if !this.composition.is_empty() {
                    this.unsent_text = Some(std::mem::take(&mut this.composition));
                    this.composition_selection = 0..0;
                    this.status = Some("Composition retained as unsent input.".into());
                }
                cx.emit(GridEvent::FocusControls);
                cx.stop_propagation();
            }))
            .on_key_down(cx.listener(Self::key_down))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    this.focus(window, cx);
                    let position = this.position(event.position);
                    this.selection = Some((position, position));
                    this.dragging = true;
                    cx.emit(GridEvent::Selected);
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                if this.dragging
                    && let Some((start, _)) = this.selection
                {
                    this.selection = Some((start, this.position(event.position)));
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.dragging = false),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.dragging = false),
            )
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _, cx| {
                let lines = match event.delta {
                    ScrollDelta::Lines(delta) => delta.y,
                    ScrollDelta::Pixels(delta) => delta.y / this.cell_size.height,
                };
                this.scroll_fraction += lines;
                let whole = this.scroll_fraction.trunc() as i32;
                this.scroll_fraction -= whole as f32;
                this.session.scroll(whole);
                this.selection = None;
                cx.stop_propagation();
                this.poll(cx);
            }))
            .child(paint::GridElement { grid: cx.entity() });
        root = root.child(body);
        match &self.session_status {
            SessionStatus::Exited(code) => {
                root=root.child(div().p_2().child(format!("Shell exited with code {code}. Its output is retained; use Restart shell to launch again.")));
            }
            SessionStatus::Failed(error) => {
                root = root.child(div().p_2().child(format!(
                    "Shell unavailable. {error} Use Restart shell to try again."
                )));
            }
            SessionStatus::Closed => {
                root = root.child(
                    div()
                        .p_2()
                        .child("Shell closed. Use Restart shell to launch again."),
                );
            }
            _ => {}
        }
        if let Some(status) = &self.status {
            root = root.child(div().text_size(zpx(12.0)).p_1().child(status.clone()));
        }
        if self.unsent_text.is_some() {
            root = root.child(
                Button::new("copy-unsent-terminal-input")
                    .label("Copy unsent input")
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(text) = this.unsent_text.take() {
                            cx.write_to_clipboard(ClipboardItem::new_string(text));
                        }
                        cx.notify();
                    })),
            );
        }
        root
    }
}

#[cfg(test)]
mod tests {
    use super::native_text;
    #[test]
    fn alt_commands_remain_distinct_from_altgr_option_and_committed_unicode_text() {
        assert!(!native_text(Some("f"), false, true, false));
        assert!(!native_text(Some("C"), true, false, false));
        assert!(native_text(Some("@"), true, true, false));
        assert!(native_text(Some("é"), false, true, false));
        assert!(native_text(Some("å"), false, true, true));
        assert!(native_text(Some("a"), false, false, false));
        assert!(!native_text(None, false, false, false));
    }
}
