use super::*;

#[test]
fn in_flight_lease_blocks_another_windows_retirement_until_the_send_finishes() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.json");
    let mut first = PendingFile::open(&config, RecoverySlot::Tempo, 64).unwrap();
    first.persist(b"saved before send").unwrap();
    let mut other = PendingFile::open(&config, RecoverySlot::Tempo, 64).unwrap();
    let lease = first.lease_pending().unwrap();
    lease.check().unwrap();
    assert_eq!(other.retire(), Err(StorageError::Busy));
    assert_eq!(other.persist(b"saved before send"), Err(StorageError::Busy));
    assert_eq!(fs::read(first.path()).unwrap(), b"saved before send");
    drop(lease);
    other.retire().unwrap();
    assert!(!first.path().exists());
}

#[test]
fn a_dispatch_lease_requires_persisted_unchanged_recovery() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.json");
    let mut file = PendingFile::open(&config, RecoverySlot::Tempo, 64).unwrap();
    assert!(matches!(
        file.lease_pending(),
        Err(StorageError::InvalidCheckpoint)
    ));
    file.persist(b"saved before send").unwrap();
    fs::write(file.path(), b"changed in another window").unwrap();
    assert!(matches!(file.lease_pending(), Err(StorageError::Changed)));
    assert_eq!(fs::read(file.path()).unwrap(), b"changed in another window");
    assert_eq!(
        PendingFile::open(&config, RecoverySlot::Tempo, 64)
            .unwrap()
            .current(),
        Some(b"changed in another window".as_slice())
    );
}

#[test]
fn providers_in_one_profile_have_separate_recovery_slots_and_cleanup() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.json");
    let mut todoist = PendingFile::open(&config, RecoverySlot::Todoist, 64).unwrap();
    let mut tempo = PendingFile::open(&config, RecoverySlot::Tempo, 64).unwrap();
    todoist.persist(b"todoist pending").unwrap();
    tempo.persist(b"tempo pending").unwrap();
    assert_ne!(todoist.path(), tempo.path());
    assert!(
        todoist
            .path()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("native-todoist-change-")
    );
    assert!(
        tempo
            .path()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("native-tempo-change-")
    );
    drop(tempo);
    let restored = PendingFile::open(&config, RecoverySlot::Tempo, 64).unwrap();
    assert_eq!(restored.current(), Some(b"tempo pending".as_slice()));
    todoist.retire().unwrap();
    assert_eq!(fs::read(restored.path()).unwrap(), b"tempo pending");
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[test]
fn each_open_captures_its_protocol_limit_and_preserves_an_oversized_existing_record() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.json");
    let mut small = PendingFile::open(&config, RecoverySlot::Todoist, 4).unwrap();
    let mut larger = PendingFile::open(&config, RecoverySlot::Tempo, 8).unwrap();
    assert_eq!(small.persist(b"12345"), Err(StorageError::TooLarge));
    assert!(!small.path().exists());
    larger.persist(b"12345").unwrap();
    assert!(matches!(
        PendingFile::open(&config, RecoverySlot::Tempo, 4),
        Err(StorageError::TooLarge)
    ));
    assert_eq!(fs::read(larger.path()).unwrap(), b"12345");
    assert!(matches!(
        PendingFile::open(&config, RecoverySlot::Tempo, 0),
        Err(StorageError::InvalidCheckpoint)
    ));
    assert!(matches!(
        PendingFile::open(&config, RecoverySlot::Tempo, 8 * 1024 * 1024 + 1),
        Err(StorageError::InvalidCheckpoint)
    ));
}

#[test]
fn one_providers_exclusive_lease_does_not_block_the_other_providers_slot() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.json");
    let todoist = PendingFile::open(&config, RecoverySlot::Todoist, 64).unwrap();
    let mut tempo = PendingFile::open(&config, RecoverySlot::Tempo, 64).unwrap();
    todoist
        .while_empty(|| tempo.persist(b"tempo pending"))
        .unwrap()
        .unwrap();
    assert_eq!(fs::read(tempo.path()).unwrap(), b"tempo pending");
    assert!(todoist.current().is_none() && !todoist.path().exists());
}

#[test]
fn terminal_append_respects_the_captured_limit_without_replacing_pending_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.json");
    let mut file = PendingFile::open(&config, RecoverySlot::Tempo, 10).unwrap();
    file.persist(b"12345678").unwrap();
    assert_eq!(
        file.append_acknowledgement(b"{}x"),
        Err(StorageError::TooLarge)
    );
    assert_eq!(fs::read(file.path()).unwrap(), b"12345678");
    file.append_acknowledgement(b"{}").unwrap();
    assert_eq!(file.current(), Some(b"12345678{}".as_slice()));
    assert_eq!(fs::read(file.path()).unwrap(), b"12345678{}");
}
