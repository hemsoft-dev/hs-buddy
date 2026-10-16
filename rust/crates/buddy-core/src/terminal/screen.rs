use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, Osc52, Term, TermMode};
use alacritty_terminal::vte::ansi::{Color, Processor};
use unicode_width::UnicodeWidthChar;

use super::output::{CHUNK_BYTES, OutputGuard, Piece};

const MAX_REPLY_BYTES: usize = 64 * 1024;
const HISTORY_LINES: usize = 2000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalSize {
    columns: u16,
    rows: u16,
}

impl TerminalSize {
    pub fn new(columns: u16, rows: u16) -> Result<Self, &'static str> {
        if !(2..=240).contains(&columns) || !(2..=100).contains(&rows) {
            return Err("Terminal size must be 2 to 240 columns and 2 to 100 rows.");
        }
        Ok(Self { columns, rows })
    }

    pub fn columns(self) -> u16 {
        self.columns
    }

    pub fn rows(self) -> u16 {
        self.rows
    }
}

impl Dimensions for TerminalSize {
    fn total_lines(&self) -> usize {
        self.rows as usize
    }
    fn screen_lines(&self) -> usize {
        self.rows as usize
    }
    fn columns(&self) -> usize {
        self.columns as usize
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellColor {
    Palette(u16),
    Rgb(u8, u8, u8),
}

impl From<Color> for CellColor {
    fn from(color: Color) -> Self {
        match color {
            Color::Named(color) => Self::Palette(color as u16),
            Color::Indexed(index) => Self::Palette(index as u16),
            Color::Spec(color) => Self::Rgb(color.r, color.g, color.b),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellStyle {
    pub foreground: CellColor,
    pub background: CellColor,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
    pub hidden: bool,
    pub dim: bool,
    pub strikeout: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenCell {
    pub row: usize,
    pub column: usize,
    pub text: String,
    pub width: usize,
    pub style: CellStyle,
}

#[derive(Clone, Debug)]
pub struct ScreenSnapshot {
    pub size: TerminalSize,
    pub cells: Vec<ScreenCell>,
    pub wrapped_rows: Vec<bool>,
    pub cursor: Option<(usize, usize)>,
    pub title: String,
    pub scroll_offset: usize,
    pub bracketed_paste: bool,
    pub application_cursor: bool,
    pub colors: Vec<Option<[u8; 3]>>,
    pub response_overflow: bool,
    pub output_limited: bool,
    pub working_directory: Option<std::path::PathBuf>,
}

impl ScreenSnapshot {
    /// Copy selected physical cells, joining soft wraps and preserving hard newlines.
    /// Wide cells are copied once even when the drag begins on their trailing cell.
    pub fn selected_text(&self, selection: std::ops::Range<usize>) -> String {
        let columns = usize::from(self.size.columns());
        let mut result = String::new();
        let rows = usize::from(self.size.rows());
        for row in 0..rows {
            let start = row * columns;
            let end = start + columns;
            if selection.start >= end || selection.end <= start {
                continue;
            }
            let mut line = String::new();
            for cell in self.cells.iter().filter(|cell| cell.row == row) {
                let offset = start + cell.column;
                if offset < selection.end && offset + cell.width > selection.start {
                    line.push_str(&cell.text);
                }
            }
            if self.wrapped_rows[row] {
                result.push_str(&line);
            } else {
                result.push_str(line.trim_end());
                if selection.end > end && row + 1 < rows {
                    result.push('\n');
                }
            }
        }
        result
    }

    /// Accessible/copy representation of the visible grid, retaining wide-cell coordinates.
    pub fn text(&self) -> String {
        let mut lines = vec![String::new(); self.size.rows as usize];
        for cell in &self.cells {
            lines[cell.row].push_str(&cell.text);
        }
        let mut result = String::new();
        for (row, line) in lines.iter().enumerate() {
            if self.wrapped_rows[row] {
                result.push_str(line);
            } else {
                result.push_str(line.trim_end());
                if row + 1 < lines.len() {
                    result.push('\n');
                }
            }
        }
        result
    }
}

struct EventState {
    replies: VecDeque<Vec<u8>>,
    bytes: usize,
    title: String,
    size: TerminalSize,
    overflow: bool,
}

#[derive(Clone)]
struct Listener(Arc<Mutex<EventState>>);

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        let mut state = self.0.lock().unwrap_or_else(|error| error.into_inner());
        let response = match event {
            Event::Title(title) => {
                state.title = title
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(256)
                    .collect();
                None
            }
            Event::ResetTitle => {
                state.title.clear();
                None
            }
            Event::PtyWrite(text) => Some(text.into_bytes()),
            Event::TextAreaSizeRequest(format) => Some(
                format(WindowSize {
                    num_lines: state.size.rows,
                    num_cols: state.size.columns,
                    cell_width: 0,
                    cell_height: 0,
                })
                .into_bytes(),
            ),
            // Remote output must never read or replace the user's clipboard, launch a URL,
            // or change application/window state. Clipboard actions are explicit UI input.
            _ => None,
        };
        if let Some(response) = response {
            if state.bytes.saturating_add(response.len()) > MAX_REPLY_BYTES {
                state.overflow = true;
            } else {
                state.bytes += response.len();
                state.replies.push_back(response);
            }
        }
    }
}

pub struct TerminalScreen {
    term: Term<Listener>,
    parser: Processor,
    listener: Listener,
    size: TerminalSize,
    output: OutputGuard,
}

impl TerminalScreen {
    pub(super) fn size(&self) -> TerminalSize {
        self.size
    }

    pub(super) fn response_overflow(&self) -> bool {
        self.listener
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .overflow
    }
    pub(super) fn input_modes(&self) -> (bool, bool) {
        (
            self.term.mode().contains(TermMode::BRACKETED_PASTE),
            self.term.mode().contains(TermMode::APP_CURSOR),
        )
    }
    pub fn new(size: TerminalSize) -> Self {
        let listener = Listener(Arc::new(Mutex::new(EventState {
            replies: VecDeque::new(),
            bytes: 0,
            title: String::new(),
            size,
            overflow: false,
        })));
        let config = Config {
            scrolling_history: HISTORY_LINES,
            osc52: Osc52::Disabled,
            ..Config::default()
        };
        Self {
            term: Term::new(config, &size, listener.clone()),
            parser: Processor::new(),
            listener,
            size,
            output: OutputGuard::default(),
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(CHUNK_BYTES) {
            for piece in self.output.chunk(chunk) {
                match piece {
                    Piece::Bytes(bytes) => self.parser.advance(&mut self.term, &bytes),
                    Piece::Character(character) => {
                        if character.width() == Some(0) {
                            let grid = self.term.grid();
                            let line = grid.cursor.point.line;
                            let mut column = grid.cursor.point.column;
                            if !grid.cursor.input_needs_wrap {
                                column.0 = column.0.saturating_sub(1);
                            }
                            if grid[line][column].flags.contains(Flags::WIDE_CHAR_SPACER) {
                                column.0 = column.0.saturating_sub(1);
                            }
                            if grid[line][column]
                                .zerowidth()
                                .is_some_and(|marks| marks.len() >= 32)
                            {
                                self.output.limited = true;
                                continue;
                            }
                        }
                        let mut bytes = [0; 4];
                        self.parser
                            .advance(&mut self.term, character.encode_utf8(&mut bytes).as_bytes());
                    }
                }
            }
        }
    }

    pub fn resize(&mut self, size: TerminalSize) {
        self.listener
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .size = size;
        self.term.resize(size);
        self.size = size;
    }

    pub fn scroll(&mut self, lines: i32) {
        self.term.scroll_display(Scroll::Delta(lines));
    }

    pub fn scroll_to_bottom(&mut self) {
        self.term.scroll_display(Scroll::Bottom);
    }

    /// Replies remain bounded until the session accepts them into its actual PTY input budget.
    pub fn take_replies(&mut self) -> Vec<Vec<u8>> {
        let mut state = self.listener.0.lock().unwrap_or_else(|e| e.into_inner());
        state.bytes = 0;
        state.replies.drain(..).collect()
    }

    pub fn snapshot(&self) -> ScreenSnapshot {
        let content = self.term.renderable_content();
        let offset = content.display_offset as i32;
        let mut cells = Vec::with_capacity(self.size.columns as usize * self.size.rows as usize);
        let mut wrapped_rows = vec![false; self.size.rows as usize];
        for indexed in content.display_iter {
            let row = indexed.point.line.0 + offset;
            if row < 0 || row >= self.size.rows as i32 {
                continue;
            }
            let cell = indexed.cell;
            if cell.flags.contains(Flags::WRAPLINE) {
                wrapped_rows[row as usize] = true;
            }
            if cell
                .flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
            {
                continue;
            }
            let mut text = cell.c.to_string();
            if let Some(combining) = cell.zerowidth() {
                text.extend(combining.iter());
            }
            cells.push(ScreenCell {
                row: row as usize,
                column: indexed.point.column.0,
                text,
                width: if cell.flags.contains(Flags::WIDE_CHAR) {
                    2
                } else {
                    1
                },
                style: CellStyle {
                    foreground: cell.fg.into(),
                    background: cell.bg.into(),
                    bold: cell.flags.contains(Flags::BOLD),
                    italic: cell.flags.contains(Flags::ITALIC),
                    underline: cell.flags.intersects(Flags::ALL_UNDERLINES),
                    inverse: cell.flags.contains(Flags::INVERSE),
                    hidden: cell.flags.contains(Flags::HIDDEN),
                    dim: cell.flags.contains(Flags::DIM),
                    strikeout: cell.flags.contains(Flags::STRIKEOUT),
                },
            });
        }
        let cursor_row = content.cursor.point.line.0 + offset;
        let cursor = (content.mode.contains(TermMode::SHOW_CURSOR)
            && (0..self.size.rows as i32).contains(&cursor_row))
        .then_some((cursor_row.max(0) as usize, content.cursor.point.column.0));
        let state = self.listener.0.lock().unwrap_or_else(|e| e.into_inner());
        ScreenSnapshot {
            size: self.size,
            cells,
            wrapped_rows,
            cursor,
            title: state.title.clone(),
            scroll_offset: content.display_offset,
            bracketed_paste: content.mode.contains(TermMode::BRACKETED_PASTE),
            application_cursor: content.mode.contains(TermMode::APP_CURSOR),
            colors: (0..269)
                .map(|i| content.colors[i].map(|c| [c.r, c.g, c.b]))
                .collect(),
            response_overflow: state.overflow,
            output_limited: self.output.limited,
            working_directory: self.output.cwd.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::vte::ansi::NamedColor;

    fn screen() -> TerminalScreen {
        TerminalScreen::new(TerminalSize::new(40, 4).unwrap())
    }

    #[test]
    fn fragmented_utf8_ansi_and_combining_characters_keep_cell_coordinates() {
        let mut screen = screen();
        let data = "\x1b[31;1mRED\x1b[0m e\u{301}界".as_bytes();
        for byte in data {
            screen.feed(&[*byte]);
        }
        let snapshot = screen.snapshot();
        assert!(snapshot.text().starts_with("RED e\u{301}界"));
        let first = &snapshot.cells[0];
        assert_eq!(
            first.style.foreground,
            CellColor::Palette(NamedColor::Red as u16)
        );
        assert!(first.style.bold);
        let wide = snapshot.cells.iter().find(|c| c.text == "界").unwrap();
        assert_eq!((wide.column, wide.width), (5, 2));
        assert!(!snapshot.cells.iter().any(|c| c.row == 0 && c.column == 6));
    }

    #[test]
    fn alternate_screen_restores_content_and_real_resize_preserves_main_grid() {
        let mut screen = screen();
        screen.feed(b"MAIN\x1b[?1049h\x1b[HALTERNATE");
        assert!(screen.snapshot().text().starts_with("ALTERNATE"));
        screen.feed(b"\x1b[?1049l");
        screen.resize(TerminalSize::new(60, 8).unwrap());
        assert!(screen.snapshot().text().starts_with("MAIN"));
        assert_eq!(screen.snapshot().size, TerminalSize::new(60, 8).unwrap());
    }

    #[test]
    fn scrollback_is_available_and_input_can_return_to_live_output() {
        let mut screen = screen();
        for line in 0..15 {
            screen.feed(format!("line-{line}\r\n").as_bytes());
        }
        let live = screen.snapshot().text();
        screen.scroll(5);
        assert_eq!(screen.snapshot().scroll_offset, 5);
        assert_ne!(screen.snapshot().text(), live);
        screen.scroll_to_bottom();
        assert_eq!(screen.snapshot().text(), live);
    }

    #[test]
    fn terminal_modes_and_device_responses_come_from_actual_parser() {
        let mut screen = screen();
        screen.feed(b"\x1b[?2004h\x1b[?1h\x1b[6n");
        assert!(screen.snapshot().bracketed_paste);
        assert!(screen.snapshot().application_cursor);
        assert_eq!(screen.take_replies(), vec![b"\x1b[1;1R".to_vec()]);
        screen.feed(b"\x1b[?2004l\x1b[?1l");
        assert!(!screen.snapshot().bracketed_paste);
        assert!(!screen.snapshot().application_cursor);
    }

    #[test]
    fn hostile_response_requests_and_remote_clipboard_are_bounded() {
        let mut screen = screen();
        screen.feed(b"\x1b]52;c;aGVsbG8=\x07\x1b]52;c;?\x07");
        assert!(screen.take_replies().is_empty());
        for _ in 0..20_000 {
            screen.feed(b"\x1b[6n");
        }
        assert!(screen.snapshot().response_overflow);
        assert!(screen.take_replies().iter().map(Vec::len).sum::<usize>() <= MAX_REPLY_BYTES);
    }

    #[test]
    fn invalid_dimensions_cannot_allocate_unbounded_grids() {
        for (cols, rows) in [(0, 24), (80, 0), (241, 24), (80, 101), (u16::MAX, u16::MAX)] {
            assert!(TerminalSize::new(cols, rows).is_err());
        }
    }

    #[test]
    fn combining_mark_storage_is_bounded_even_when_escape_sequences_revisit_the_cell() {
        let mut screen = screen();
        screen.feed(b"e");
        for _ in 0..10_000 {
            screen.feed("\x1b[1;2H\u{301}".as_bytes());
        }
        let snapshot = screen.snapshot();
        assert!(snapshot.output_limited);
        assert_eq!(snapshot.cells[0].text.chars().count(), 33);
        screen.feed(b"\x1b[2;1HSTILL USABLE");
        assert!(screen.snapshot().text().contains("STILL USABLE"));
    }

    #[test]
    fn copy_text_joins_soft_wraps_and_preserves_hard_line_breaks() {
        let mut screen = TerminalScreen::new(TerminalSize::new(10, 4).unwrap());
        screen.feed(b"long-directory-name\r\nnext");
        assert!(
            screen
                .snapshot()
                .text()
                .starts_with("long-directory-name\nnext")
        );
    }

    #[test]
    fn partial_selection_copies_wide_cells_once_and_distinguishes_soft_and_hard_wraps() {
        let mut screen = TerminalScreen::new(TerminalSize::new(6, 3).unwrap());
        screen.feed("a界bcdef\r\nnext".as_bytes());
        let snapshot = screen.snapshot();
        assert_eq!(snapshot.selected_text(2..8), "界bcdef");
        assert_eq!(snapshot.selected_text(2..16), "界bcdef\nnext");
        assert_eq!(snapshot.selected_text(5..5), "");
    }
}
