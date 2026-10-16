use std::ops::Range;

use buddy_core::terminal::{Key, MAX_PASTE_BYTES, Modifiers};
use gpui_kit::{Bounds, Context, EntityInputHandler, Pixels, Point, UTF16Selection, Window};

use super::TerminalGrid;

fn utf16_slice(text: &str, range: Range<usize>) -> Option<String> {
    let units = text.encode_utf16().collect::<Vec<_>>();
    String::from_utf16(units.get(range)?).ok()
}

fn replacement(text: &str, range: Option<Range<usize>>, new: &str) -> Option<String> {
    let Some(range) = range else {
        return Some(new.into());
    };
    if range.start > range.end {
        return None;
    }
    let length = text.encode_utf16().count();
    Some(utf16_slice(text, 0..range.start)? + new + &utf16_slice(text, range.end..length)?)
}

impl TerminalGrid {
    fn commit_text(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.composition.clear();
        self.composition_selection = 0..0;
        if text.is_empty() {
            cx.notify();
            return;
        }
        if !self.visible
            || !self.selected
            || !self.focus.is_focused(window)
            || self.confirmation.is_some()
        {
            self.unsent_text = Some(text);
            self.status =
                Some("The input target changed. Text was retained without being sent.".into());
            cx.notify();
            return;
        }
        if text.contains(['\n', '\r']) {
            self.paste(text, None, false, cx);
            return;
        }
        if text.len() > MAX_PASTE_BYTES || text.chars().any(char::is_control) {
            self.status = Some(
                "This committed input cannot be sent as terminal text. It is retained below."
                    .into(),
            );
            self.unsent_text = Some(text);
        } else if let Err(error) = self.session.key(Key::Text(&text), Modifiers::default()) {
            self.status = Some(error.to_string());
            self.unsent_text = Some(text);
        }
        self.selection = None;
        cx.notify();
    }
}

impl EntityInputHandler for TerminalGrid {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let text = utf16_slice(&self.composition, range.clone())?;
        *adjusted = Some(range);
        Some(text)
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.composition_selection.clone(),
            reversed: false,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        (!self.composition.is_empty()).then(|| 0..self.composition.encode_utf16().count())
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = std::mem::take(&mut self.composition);
        self.commit_text(text, window, cx);
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(text) = replacement(&self.composition, range, text) {
            self.commit_text(text, window, cx);
        } else {
            self.status = Some(
                "The input method supplied an unsupported text range. No text was sent.".into(),
            );
            cx.notify();
        }
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selection: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(text) = replacement(&self.composition, range, text) else {
            return;
        };
        if text.len() > MAX_PASTE_BYTES {
            self.status=Some("The composition is too large. The prior composition is retained; no text was sent.".into());
            cx.notify();
            return;
        }
        let length = text.encode_utf16().count();
        self.composition = text;
        self.composition_selection = selection
            .filter(|range| range.start <= range.end && range.end <= length)
            .unwrap_or(length..length);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let bounds = self.bounds.unwrap_or(bounds);
        let (row, column) = self.screen.cursor?;
        Some(Bounds::new(
            gpui_kit::point(
                bounds.left() + self.cell_size.width * column as f32,
                bounds.top() + self.cell_size.height * row as f32,
            ),
            self.cell_size,
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let bounds = self.bounds?;
        (point.x >= bounds.left()
            && point.x <= bounds.right()
            && point.y >= bounds.top()
            && point.y <= bounds.bottom())
        .then(|| self.composition.encode_utf16().count())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_ranges_count_utf16_units_and_never_split_surrogate_pairs() {
        assert_eq!(utf16_slice("a😀界", 1..3).as_deref(), Some("😀"));
        assert!(utf16_slice("a😀界", 1..2).is_none());
        assert_eq!(
            replacement("a😀界", Some(1..3), "é").as_deref(),
            Some("aé界")
        );
        assert!(replacement("a😀界", Some(9..10), "x").is_none());
        let reversed = Range { start: 3, end: 1 };
        assert!(replacement("a😀界", Some(reversed), "x").is_none());
    }
}
