//! Bound control-string storage before the upstream std-enabled VTE parser sees it.
use std::path::PathBuf;

const MAX_OSC_BYTES: usize = 8192;
pub(super) const CHUNK_BYTES: usize = 4096;

pub(super) enum Piece {
    Bytes(Vec<u8>),
    Character(char),
}

#[derive(Default)]
pub(super) struct OutputGuard {
    escape: bool,
    osc: Option<Vec<u8>>,
    osc_escape: bool,
    osc_overflow: bool,
    utf8: Vec<u8>,
    pub limited: bool,
    pub cwd: Option<PathBuf>,
}

fn flush(bytes: &mut Vec<u8>, output: &mut Vec<Piece>) {
    if !bytes.is_empty() {
        output.push(Piece::Bytes(std::mem::take(bytes)));
    }
}

impl OutputGuard {
    /// Caller chunks first, so this transient result is bounded even for arbitrarily long output.
    pub fn chunk(&mut self, bytes: &[u8]) -> Vec<Piece> {
        assert!(bytes.len() <= CHUNK_BYTES);
        let mut output = Vec::new();
        let mut plain = Vec::new();
        for &byte in bytes {
            if self.osc.is_some() {
                if matches!(byte, 0x18 | 0x1a) {
                    self.osc = None;
                    self.osc_escape = false;
                    self.osc_overflow = false;
                    output.push(Piece::Bytes(vec![byte]));
                    continue;
                }
                let finished = byte == 7 || (self.osc_escape && byte == b'\\');
                self.osc_escape = byte == 0x1b;
                let osc = self.osc.as_mut().expect("OSC exists");
                if osc.len() < MAX_OSC_BYTES {
                    osc.push(byte);
                } else {
                    self.osc_overflow = true;
                    self.limited = true;
                }
                if finished {
                    let mut osc = self.osc.take().expect("OSC exists");
                    if !self.osc_overflow {
                        if byte == 7 {
                            osc.pop();
                        } else {
                            osc.truncate(osc.len().saturating_sub(2));
                        }
                        self.complete_osc(osc, &mut output);
                    }
                    self.osc_overflow = false;
                    self.osc_escape = false;
                }
                continue;
            }
            if self.escape {
                self.escape = false;
                if byte == b']' {
                    flush(&mut plain, &mut output);
                    output.push(Piece::Bytes(vec![0x18]));
                    self.osc = Some(Vec::new());
                    continue;
                }
                plain.push(0x1b);
            }
            if byte == 0x1b {
                if !self.utf8.is_empty() {
                    plain.append(&mut self.utf8);
                }
                self.escape = true;
                continue;
            }
            if self.utf8.is_empty() && byte < 0x80 {
                plain.push(byte);
                continue;
            }
            if !self.utf8.is_empty() && byte & 0xc0 != 0x80 {
                plain.append(&mut self.utf8);
            }
            if byte < 0x80 {
                plain.push(byte);
                continue;
            }
            self.utf8.push(byte);
            let expected = match self.utf8[0] {
                0xc2..=0xdf => 2,
                0xe0..=0xef => 3,
                0xf0..=0xf4 => 4,
                _ => 1,
            };
            if self.utf8.len() >= expected {
                flush(&mut plain, &mut output);
                match std::str::from_utf8(&self.utf8)
                    .ok()
                    .and_then(|s| s.chars().next())
                {
                    Some(character) => output.push(Piece::Character(character)),
                    None => output.push(Piece::Bytes(self.utf8.clone())),
                }
                self.utf8.clear();
            }
        }
        flush(&mut plain, &mut output);
        output
    }

    fn complete_osc(&mut self, osc: Vec<u8>, output: &mut Vec<Piece>) {
        // The renderer never activates terminal hyperlinks and remote output cannot access
        // clipboard contents. Avoid retaining per-cell hyperlink allocations at all.
        if osc.starts_with(b"8;") || osc.starts_with(b"52;") {
            return;
        }
        if let Some(path) = osc.strip_prefix(b"7;").and_then(local_file_url) {
            self.cwd = Some(path);
            return;
        }
        let mut complete = Vec::with_capacity(osc.len() + 3);
        complete.extend_from_slice(b"\x1b]");
        complete.extend_from_slice(&osc);
        complete.push(7);
        output.push(Piece::Bytes(complete));
    }
}

fn local_file_url(bytes: &[u8]) -> Option<PathBuf> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut url = reqwest::Url::parse(text).ok()?;
    if url.scheme() != "file"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let host = url.host_str().unwrap_or("");
    let local = host.is_empty()
        || host.eq_ignore_ascii_case("localhost")
        || ["COMPUTERNAME", "HOSTNAME"]
            .iter()
            .any(|key| std::env::var(key).is_ok_and(|value| value.eq_ignore_ascii_case(host)));
    if !local {
        return None;
    }
    url.set_host(None).ok()?;
    let path = url.to_file_path().ok()?;
    let text = path.to_str()?;
    (path.is_absolute() && text.len() <= 4096 && !text.contains('\0')).then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unterminated_osc_never_grows_beyond_the_buffer_limit() {
        let mut guard = OutputGuard::default();
        guard.chunk(b"\x1b]");
        for _ in 0..3000 {
            assert!(guard.chunk(&[b'x'; CHUNK_BYTES]).is_empty());
        }
        assert_eq!(guard.osc.as_ref().unwrap().len(), MAX_OSC_BYTES);
        assert!(guard.limited);
        let output = guard.chunk(b"\x07VISIBLE");
        assert!(matches!(&output[0],Piece::Bytes(value) if value==b"VISIBLE"));
        assert!(guard.osc.is_none());
    }

    #[test]
    fn remote_file_urls_and_control_bearing_paths_cannot_become_local_directories() {
        assert!(local_file_url(b"file://unrelated.invalid/tmp/work").is_none());
        assert!(local_file_url(b"file:///tmp/zero%00path").is_none());
        assert!(local_file_url(b"https://localhost/tmp/work").is_none());
    }
}
