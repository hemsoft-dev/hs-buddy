//! Added and removed line counts between two versions of a text file.

/// Files with more lines than this get no count.
const MAX_LINES: usize = 100_000;

/// Like Git, a file with a NUL byte near its start is binary.
fn binary(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(8000)].contains(&0)
}

/// Lines with their terminators, so a missing final newline counts as a change, as in Git.
fn lines(bytes: &[u8]) -> Vec<&[u8]> {
    bytes.split_inclusive(|byte| *byte == b'\n').collect()
}

/// Longest common subsequence length, in `O(n·m)` time and `O(m)` memory.
fn common(old: &[&[u8]], new: &[&[u8]]) -> usize {
    let mut previous = vec![0usize; new.len() + 1];
    let mut row = vec![0usize; new.len() + 1];
    for line in old {
        for (index, other) in new.iter().enumerate() {
            row[index + 1] = if line == other {
                previous[index] + 1
            } else {
                row[index].max(previous[index + 1])
            };
        }
        std::mem::swap(&mut previous, &mut row);
    }
    previous[new.len()]
}

/// `(added, removed)` lines, or `None` for binary content and very large rewrites.
pub fn count(old: &[u8], new: &[u8]) -> Option<(u32, u32)> {
    if binary(old) || binary(new) {
        return None;
    }
    // Counted before any per-line allocation: a file of mostly newlines is not counted.
    let newlines = |bytes: &[u8]| bytes.iter().filter(|byte| **byte == b'\n').count();
    if newlines(old) > MAX_LINES || newlines(new) > MAX_LINES {
        return None;
    }
    let (old, new) = (lines(old), lines(new));
    let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
    let (old, new) = (&old[prefix..], &new[prefix..]);
    let suffix = old
        .iter()
        .rev()
        .zip(new.iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let (old, new) = (&old[..old.len() - suffix], &new[..new.len() - suffix]);
    // Beyond about four million cells an exact count takes too long; show none.
    if old.len().saturating_mul(new.len()) > 4_000_000 {
        return None;
    }
    let kept = common(old, new);
    let clamp = |value: usize| u32::try_from(value).unwrap_or(u32::MAX);
    Some((clamp(new.len() - kept), clamp(old.len() - kept)))
}

#[cfg(test)]
mod tests {
    use super::count;

    #[test]
    fn counts_match_a_line_diff() {
        assert_eq!(count(b"", b""), Some((0, 0)));
        assert_eq!(count(b"", b"a\nb\n"), Some((2, 0)));
        assert_eq!(count(b"a\nb\n", b""), Some((0, 2)));
        assert_eq!(count(b"a\nb\nc\n", b"a\nB\nc\n"), Some((1, 1)));
        assert_eq!(count(b"a\nb\nc\n", b"a\nc\nb\n"), Some((1, 1)));
        assert_eq!(count(b"a\nb\n", b"a\nb\nc\nd\n"), Some((2, 0)));
        // Git counts an added final newline as one changed line.
        assert_eq!(count(b"a\nb", b"a\nb\n"), Some((1, 1)));
        assert_eq!(count(b"a\r\nb\r\n", b"a\nb\n"), Some((2, 2)));
        assert_eq!(count(b"x\0y", b"x"), None);
    }

    #[test]
    fn very_large_rewrites_have_no_count() {
        let old: Vec<u8> = (0..3000)
            .flat_map(|n| format!("{n}\n").into_bytes())
            .collect();
        let new: Vec<u8> = (0..3000)
            .rev()
            .flat_map(|n| format!("{n}\n").into_bytes())
            .collect();
        assert_eq!(count(&old, &new), None);
        // A small edit inside a large file is still counted exactly.
        let mut edited = old.clone();
        edited.splice(0..2, b"zero\n".iter().copied());
        assert_eq!(count(&old, &edited), Some((1, 1)));
    }

    #[test]
    fn newline_floods_are_not_counted() {
        let flood = vec![b'\n'; super::MAX_LINES + 1];
        assert_eq!(count(b"", &flood), None);
        assert_eq!(count(&flood, b"x\n"), None);
        let most = vec![b'\n'; super::MAX_LINES];
        assert_eq!(count(b"", &most), Some((super::MAX_LINES as u32, 0)));
    }
}
