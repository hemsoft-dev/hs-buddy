//! SHA-256 output as lowercase hex, and hashing from a reader. `sha2` 0.11
//! dropped the `{:x}` formatting and the `io::Write` impl its 0.10 types had.

use sha2::{Digest as _, Sha256};

/// Lowercase hex of a digest, as `format!("{:x}", digest)` printed in `sha2` 0.10.
pub(crate) fn hex(digest: &[u8]) -> String {
    use std::fmt::Write as _;
    digest
        .iter()
        .fold(String::with_capacity(digest.len() * 2), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

/// Lowercase hex SHA-256 of `bytes`.
pub(crate) fn hex_digest(bytes: impl AsRef<[u8]>) -> String {
    hex(&Sha256::digest(bytes))
}

/// Lets `io::copy` feed a hasher.
pub(crate) struct Writer<'a>(pub &'a mut Sha256);

impl std::io::Write for Writer<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    #[test]
    fn hex_matches_the_published_sha256_of_abc() {
        assert_eq!(hex_digest(b"abc"), ABC);
        assert_eq!(hex(&[0x00, 0x0f, 0xa0]), "000fa0");
    }

    #[test]
    fn a_copied_reader_hashes_like_its_bytes() {
        let mut hasher = Sha256::new();
        let mut input: &[u8] = b"abc";
        let copied = std::io::copy(&mut input, &mut Writer(&mut hasher)).unwrap();
        assert_eq!(copied, 3);
        assert_eq!(hex(&hasher.finalize()), ABC);
    }
}
