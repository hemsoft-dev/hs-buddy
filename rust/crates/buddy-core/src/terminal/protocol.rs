//! Private same-executable protocol. Frame length is checked before allocation or decoding.
use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize, de::DeserializeOwned};

use super::{ShellSpec, TerminalSize};

pub(super) const MAX_FRAME_BYTES: usize = 512 * 1024;
pub(super) const OUTPUT_CHUNK_BYTES: usize = 16 * 1024;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub(super) enum Command {
    Start {
        shell: ShellSpec,
        columns: u16,
        rows: u16,
    },
    Input {
        id: u64,
        bytes: Vec<u8>,
    },
    Resize {
        columns: u16,
        rows: u16,
    },
    Close,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub(super) enum Event {
    Started { shell_pid: u32 },
    Output { bytes: Vec<u8> },
    InputWritten { id: u64, bytes: usize },
    Resized { columns: u16, rows: u16 },
    Exited { code: u32 },
    Error { message: String },
}

impl Command {
    pub fn size(&self) -> Result<TerminalSize, &'static str> {
        match self {
            Self::Start { columns, rows, .. } | Self::Resize { columns, rows } => {
                TerminalSize::new(*columns, *rows)
            }
            _ => Err("This message has no terminal dimensions."),
        }
    }
}

pub(super) fn read_frame<T: DeserializeOwned>(reader: &mut impl Read) -> io::Result<Option<T>> {
    let mut prefix = [0; 4];
    match reader.read(&mut prefix[..1])? {
        0 => return Ok(None),
        1 => reader.read_exact(&mut prefix[1..])?,
        _ => unreachable!("one byte buffer"),
    }
    let length = u32::from_le_bytes(prefix) as usize;
    if length == 0 || length > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Terminal frame exceeds its size limit.",
        ));
    }
    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes)?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Invalid terminal frame."))
}

pub(super) fn write_frame(writer: &mut impl Write, value: &impl Serialize) -> io::Result<()> {
    let bytes = serde_json::to_vec(value).map_err(io::Error::other)?;
    if bytes.is_empty() || bytes.len() > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Terminal frame exceeds its size limit.",
        ));
    }
    writer.write_all(&(bytes.len() as u32).to_le_bytes())?;
    writer.write_all(&bytes)?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn framing_preserves_binary_input_and_unicode_paths() {
        let frame = Command::Input {
            id: 42,
            bytes: vec![0, 0xff, 0x1b, b'\r'],
        };
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &frame).unwrap();
        let decoded: Command = read_frame(&mut Cursor::new(bytes)).unwrap().unwrap();
        assert!(matches!(decoded,Command::Input{id:42,bytes} if bytes==[0,0xff,0x1b,b'\r']));
    }

    #[test]
    fn oversize_prefix_fails_before_reading_or_allocating_the_claimed_body() {
        let mut bytes = Cursor::new((MAX_FRAME_BYTES as u32 + 1).to_le_bytes());
        assert_eq!(
            read_frame::<Command>(&mut bytes).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(bytes.position(), 4);
        assert!(read_frame::<Command>(&mut Cursor::new([2, 0, 0, 0, b'{'])).is_err());
        assert!(
            read_frame::<Command>(&mut Cursor::new([]))
                .unwrap()
                .is_none()
        );
    }
}
