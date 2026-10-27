//! Exclusive file locks that tell a real holder from a process being spawned.
//!
//! On Unix a lock belongs to the open file description, and a child process being spawned
//! on another thread (Buddy runs Git, `gh` and loops) shares every open description until
//! it executes. Right after a holder drops its lock, `try_lock` can therefore still report
//! it held for a moment. A lock counts as held only while that lasts about 200 ms.
use std::fs::{File, TryLockError};
use std::time::Duration;

const ATTEMPTS: u32 = 10;
const PAUSE: Duration = Duration::from_millis(20);

/// Takes `file`'s exclusive lock; `false` when another holder keeps it.
pub(crate) fn acquire(file: &File) -> std::io::Result<bool> {
    for attempt in 1..=ATTEMPTS {
        match file.try_lock() {
            Ok(()) => return Ok(true),
            Err(TryLockError::WouldBlock) if attempt < ATTEMPTS => std::thread::sleep(PAUSE),
            Err(TryLockError::WouldBlock) => return Ok(false),
            Err(TryLockError::Error(error)) => return Err(error),
        }
    }
    Ok(false)
}

/// Whether another holder keeps `file` locked. Taking the lock to find out releases it
/// again when the caller drops `file`.
pub(crate) fn held(file: &File) -> bool {
    matches!(acquire(file), Ok(false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lock_held_by_another_handle_is_held_and_a_dropped_one_is_free() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("lock");
        let holder = File::create(&path).unwrap();
        assert!(acquire(&holder).unwrap());
        assert!(held(&File::open(&path).unwrap()));
        drop(holder);
        let next = File::open(&path).unwrap();
        assert!(acquire(&next).unwrap());
    }

    /// The race this module exists for: many children spawned while a lock is dropped and
    /// taken again at once. A plain `try_lock` fails here now and then on Linux and macOS.
    #[cfg(unix)]
    #[test]
    fn a_lock_released_while_children_spawn_is_free() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("lock");
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let spawner = {
            let stop = stop.clone();
            std::thread::spawn(move || {
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    let _ = std::process::Command::new("true").status();
                }
            })
        };
        for _ in 0..200 {
            let holder = File::create(&path).unwrap();
            assert!(acquire(&holder).unwrap());
            drop(holder);
            let next = File::open(&path).unwrap();
            assert!(acquire(&next).unwrap(), "a dropped lock counted as held");
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        spawner.join().unwrap();
    }
}
