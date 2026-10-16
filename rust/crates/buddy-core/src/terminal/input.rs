use std::collections::VecDeque;

pub const MAX_INPUT_BYTES: usize = 256 * 1024;
pub const MAX_PASTE_BYTES: usize = 64 * 1024;
const MAX_PENDING_INPUTS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputError {
    TooLarge,
    Backpressure,
    Closed,
    InvalidControl,
    ConfirmMultiline,
    InvalidAcknowledgement,
}

impl std::fmt::Display for InputError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::TooLarge => "Paste is larger than the 64 KiB terminal input limit.",
            Self::Backpressure => "The shell is not accepting input. Wait or close this session.",
            Self::Closed => "This terminal session has closed. Start a new session before typing.",
            Self::InvalidControl => "Paste contains terminal control characters. Remove them before pasting.",
            Self::ConfirmMultiline => "This shell does not advertise bracketed paste. Lines may execute immediately. Confirm before pasting.",
            Self::InvalidAcknowledgement => "The terminal input acknowledgement is invalid. Close and restart this session.",
        })
    }
}
impl std::error::Error for InputError {}

#[derive(Default, Debug, Clone, Copy)]
pub struct Modifiers {
    pub control: bool,
    pub alt: bool,
    pub shift: bool,
}

#[derive(Debug, Clone, Copy)]
pub enum Key<'a> {
    Text(&'a str),
    Enter,
    Backspace,
    Tab,
    Escape,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    Insert,
    Delete,
    PageUp,
    PageDown,
    Function(u8),
}

/// Encode a key against the live emulator mode. IME text enters through Text after commit.
pub fn encode_key(key: Key<'_>, modifiers: Modifiers, application_cursor: bool) -> Vec<u8> {
    let modifier = 1
        + u8::from(modifiers.shift)
        + 2 * u8::from(modifiers.alt)
        + 4 * u8::from(modifiers.control);
    let cursor = |suffix: char| {
        if modifier != 1 {
            format!("\x1b[1;{modifier}{suffix}").into_bytes()
        } else if application_cursor {
            format!("\x1bO{suffix}").into_bytes()
        } else {
            format!("\x1b[{suffix}").into_bytes()
        }
    };
    let numbered = |number: u8| {
        if modifier == 1 {
            format!("\x1b[{number}~").into_bytes()
        } else {
            format!("\x1b[{number};{modifier}~").into_bytes()
        }
    };
    match key {
        Key::Up => cursor('A'),
        Key::Down => cursor('B'),
        Key::Right => cursor('C'),
        Key::Left => cursor('D'),
        Key::Home => cursor('H'),
        Key::End => cursor('F'),
        Key::Insert => numbered(2),
        Key::Delete => numbered(3),
        Key::PageUp => numbered(5),
        Key::PageDown => numbered(6),
        Key::Function(number @ 1..=4) => {
            let suffix = (b'P' + number - 1) as char;
            if modifier == 1 {
                format!("\x1bO{suffix}").into_bytes()
            } else {
                format!("\x1b[1;{modifier}{suffix}").into_bytes()
            }
        }
        Key::Function(5) => numbered(15),
        Key::Function(6) => numbered(17),
        Key::Function(7) => numbered(18),
        Key::Function(8) => numbered(19),
        Key::Function(9) => numbered(20),
        Key::Function(10) => numbered(21),
        Key::Function(11) => numbered(23),
        Key::Function(12) => numbered(24),
        Key::Function(_) => Vec::new(),
        Key::Tab if modifiers.shift => b"\x1b[Z".to_vec(),
        other => {
            let mut bytes = match other {
                Key::Enter => vec![b'\r'],
                Key::Backspace => vec![0x7f],
                Key::Tab => vec![b'\t'],
                Key::Escape => vec![0x1b],
                Key::Text(text) if modifiers.control && text.len() == 1 => {
                    let byte = text.as_bytes()[0].to_ascii_uppercase();
                    match byte {
                        b'@'..=b'_' => vec![byte & 0x1f],
                        b' ' | b'2' => vec![0],
                        b'?' | b'8' => vec![0x7f],
                        _ => return Vec::new(),
                    }
                }
                Key::Text(_) if modifiers.control => return Vec::new(),
                Key::Text(text) => text.as_bytes().to_vec(),
                _ => unreachable!("special keys handled above"),
            };
            if modifiers.alt {
                bytes.insert(0, 0x1b);
            }
            bytes
        }
    }
}

/// No Enter is appended. An unbracketed newline requires an explicit immutable-paste confirmation.
pub fn encode_paste(
    text: &str,
    bracketed: bool,
    confirmed_multiline: bool,
) -> Result<Vec<u8>, InputError> {
    if text.len() > MAX_PASTE_BYTES {
        return Err(InputError::TooLarge);
    }
    if text
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return Err(InputError::InvalidControl);
    }
    if !bracketed && !confirmed_multiline && text.contains(['\r', '\n']) {
        return Err(InputError::ConfirmMultiline);
    }
    let normalized = text.replace("\r\n", "\n").replace('\n', "\r");
    let bytes = if bracketed {
        format!("\x1b[200~{normalized}\x1b[201~").into_bytes()
    } else {
        normalized.into_bytes()
    };
    if bytes.len() > MAX_PASTE_BYTES {
        return Err(InputError::TooLarge);
    }
    Ok(bytes)
}

/// Credits remain reserved until the host confirms an actual PTY write, never just a queue send.
#[derive(Default)]
pub struct InputBudget {
    pending: VecDeque<(u64, usize)>,
    bytes: usize,
    next_id: u64,
    written_id: u64,
    closed: bool,
}

impl InputBudget {
    pub fn reserve(&mut self, bytes: usize) -> Result<u64, InputError> {
        if self.closed {
            return Err(InputError::Closed);
        }
        if bytes == 0 || bytes > MAX_PASTE_BYTES {
            return Err(InputError::TooLarge);
        }
        if self.bytes.saturating_add(bytes) > MAX_INPUT_BYTES
            || self.pending.len() >= MAX_PENDING_INPUTS
        {
            return Err(InputError::Backpressure);
        }
        self.next_id = self.next_id.checked_add(1).ok_or(InputError::Closed)?;
        self.pending.push_back((self.next_id, bytes));
        self.bytes += bytes;
        Ok(self.next_id)
    }

    pub fn acknowledge(&mut self, id: u64, bytes: usize) -> Result<(), InputError> {
        if self.pending.front() != Some(&(id, bytes)) {
            self.closed = true;
            return Err(InputError::InvalidAcknowledgement);
        }
        self.pending.pop_front();
        self.bytes -= bytes;
        self.written_id = id;
        Ok(())
    }

    /// Only a rejected, definitely-unsent last enqueue may return its reservation.
    pub fn cancel_unsent(&mut self, id: u64) -> Result<(), InputError> {
        match self.pending.back().copied() {
            Some((last, bytes)) if last == id => {
                self.pending.pop_back();
                self.bytes -= bytes;
                Ok(())
            }
            _ => Err(InputError::InvalidAcknowledgement),
        }
    }

    pub fn pending_bytes(&self) -> usize {
        self.bytes
    }

    pub(super) fn written_through(&self) -> u64 {
        self.written_id
    }

    pub fn close(&mut self) {
        self.closed = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_application_and_modified_cursor_keys_have_distinct_bytes() {
        assert_eq!(encode_key(Key::Up, Modifiers::default(), false), b"\x1b[A");
        assert_eq!(encode_key(Key::Up, Modifiers::default(), true), b"\x1bOA");
        assert_eq!(
            encode_key(
                Key::Left,
                Modifiers {
                    control: true,
                    ..Default::default()
                },
                true
            ),
            b"\x1b[1;5D"
        );
        assert_eq!(
            encode_key(
                Key::Text("c"),
                Modifiers {
                    control: true,
                    ..Default::default()
                },
                false
            ),
            [3]
        );
        assert_eq!(
            encode_key(Key::Text("界"), Modifiers::default(), false),
            "界".as_bytes()
        );
    }

    #[test]
    fn multiline_requires_mode_or_explicit_confirmation_and_never_appends_enter() {
        assert_eq!(
            encode_paste("one\ntwo", false, false),
            Err(InputError::ConfirmMultiline)
        );
        assert_eq!(
            encode_paste("one\r\ntwo", true, false).unwrap(),
            b"\x1b[200~one\rtwo\x1b[201~"
        );
        assert_eq!(encode_paste("one\ntwo", false, true).unwrap(), b"one\rtwo");
        assert_eq!(
            encode_paste("  exact text  ", false, false).unwrap(),
            b"  exact text  "
        );
        assert_eq!(
            encode_paste("\x1b[201~evil", true, true),
            Err(InputError::InvalidControl)
        );
    }

    #[test]
    fn input_credits_require_ordered_exact_write_acknowledgements() {
        let mut budget = InputBudget::default();
        let ids = (0..4)
            .map(|_| budget.reserve(MAX_PASTE_BYTES).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(budget.reserve(1), Err(InputError::Backpressure));
        assert_eq!(budget.pending_bytes(), MAX_INPUT_BYTES);
        budget.acknowledge(ids[0], MAX_PASTE_BYTES).unwrap();
        assert_eq!(budget.pending_bytes(), MAX_INPUT_BYTES - MAX_PASTE_BYTES);
        let later = budget.reserve(8).unwrap();
        budget.cancel_unsent(later).unwrap();
        assert_eq!(budget.pending_bytes(), MAX_INPUT_BYTES - MAX_PASTE_BYTES);
        assert_eq!(
            budget.acknowledge(ids[2], MAX_PASTE_BYTES),
            Err(InputError::InvalidAcknowledgement)
        );
        assert_eq!(budget.reserve(1), Err(InputError::Closed));
        assert_eq!(budget.pending_bytes(), MAX_INPUT_BYTES - MAX_PASTE_BYTES);
    }
}
