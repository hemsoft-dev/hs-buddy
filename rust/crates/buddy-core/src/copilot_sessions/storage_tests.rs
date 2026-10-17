use super::*;
use std::fs;
use tempfile::TempDir;

fn fixture() -> (TempDir, PathBuf, SessionRoot) {
    let temp = tempfile::tempdir().unwrap();
    let chat = temp.path().join("workspaceStorage/hash-界/chatSessions");
    fs::create_dir_all(&chat).unwrap();
    let root = SessionRoot::open(temp.path()).unwrap();
    (temp, chat, root)
}

fn content(id: &str) -> String {
    serde_json::json!({"kind":0,"v":{"sessionId":id,"creationDate":123,"customTitle":"Unicode 界 🦀"}}).to_string() + "\n"
}

#[test]
fn scans_owned_unicode_files_and_reads_the_selected_identity() {
    let (_temp, chat, root) = fixture();
    fs::write(chat.join("session-界.jsonl"), content("identity-界")).unwrap();
    let scan = root.scan(ScanLimits::default()).unwrap();
    assert_eq!(scan.sessions.len(), 1);
    assert_eq!(scan.sessions[0].title, "Unicode 界 🦀");
    assert!(!scan.sessions[0].preview_partial);
    assert!(!scan.truncated);
    let detail = root
        .detail(&scan.sessions[0].file, Limits::default())
        .unwrap();
    assert_eq!(detail.session_id, "identity-界");
    assert_eq!(
        fs::read_to_string(chat.join("session-界.jsonl")).unwrap(),
        content("identity-界")
    );
}

#[test]
fn scan_preview_preserves_pending_prompts_without_inventing_completed_results() {
    let (_temp, chat, root) = fixture();
    let prompt = "界🦀".repeat(150);
    let init = serde_json::json!({"kind":0,"v":{"sessionId":"pending","creationDate":456,"responderUsername":"agent-界","requests":[{"message":{"text":prompt}}]}});
    fs::write(chat.join("pending.jsonl"), init.to_string()).unwrap();
    let scan = root.scan(ScanLimits::default()).unwrap();
    let session = &scan.sessions[0];
    assert_eq!(
        session.first_prompt,
        prompt.chars().take(200).collect::<String>()
    );
    assert_eq!(session.agent, "agent-界");
    assert_eq!(session.created_at, 456);
    assert_eq!(session.preview_request_count, 1);
    assert!(
        root.detail(&session.file, Limits::default())
            .unwrap()
            .results
            .is_empty()
    );
    assert!(matches!(
        SessionRoot::open(Path::new("relative")),
        Err(StorageError::RootUnavailable)
    ));
}

#[test]
fn workspace_names_decode_unicode_folder_and_workspace_uris() {
    let (_temp, chat, root) = fixture();
    fs::write(chat.join("one.jsonl"), content("one")).unwrap();
    let workspace = chat.parent().unwrap().join("workspace.json");
    fs::write(
        &workspace,
        r#"{"folder":"file:///C%3A/Dev/My%20%E7%95%8C/"}"#,
    )
    .unwrap();
    assert_eq!(
        root.scan(ScanLimits::default()).unwrap().sessions[0].workspace_name,
        "My 界"
    );
    fs::write(
        &workspace,
        r#"{"workspace":"file:///home/test/My%20Team.code-workspace"}"#,
    )
    .unwrap();
    assert_eq!(
        root.scan(ScanLimits::default()).unwrap().sessions[0].workspace_name,
        "My Team"
    );
    fs::write(&workspace, r#"{"folder":"file:///broken%XX"}"#).unwrap();
    assert_eq!(
        root.scan(ScanLimits::default()).unwrap().sessions[0].workspace_name,
        "hash-界"
    );
}

#[test]
fn workspace_metadata_shares_the_total_read_budget_and_never_changes_source_files() {
    let (_temp, chat, root) = fixture();
    fs::write(chat.join("one.jsonl"), content("one")).unwrap();
    let workspace = chat.parent().unwrap().join("workspace.json");
    let source = " ".repeat(32 * 1024);
    fs::write(&workspace, &source).unwrap();
    let scan = root
        .scan(ScanLimits {
            total_prefix_bytes: 100,
            ..ScanLimits::default()
        })
        .unwrap();
    assert!(scan.truncated);
    assert!(scan.sessions.is_empty());
    assert_eq!(scan.preview_bytes_read, 100);
    assert_eq!(fs::read_to_string(workspace).unwrap(), source);
}

#[test]
fn keys_cannot_be_used_after_switching_sources_even_to_the_same_directory() {
    let (temp, chat, root) = fixture();
    fs::write(chat.join("one.jsonl"), content("one")).unwrap();
    let key = root
        .scan(ScanLimits::default())
        .unwrap()
        .sessions
        .remove(0)
        .file;
    let other = SessionRoot::open(temp.path()).unwrap();
    assert_eq!(
        other.detail(&key, Limits::default()),
        Err(StorageError::WrongRoot)
    );
}

#[test]
fn replacing_a_scanned_file_with_another_identity_requires_rescan() {
    let (_temp, chat, root) = fixture();
    let file = chat.join("one.jsonl");
    fs::write(&file, content("one")).unwrap();
    let key = root
        .scan(ScanLimits::default())
        .unwrap()
        .sessions
        .remove(0)
        .file;
    fs::write(file, content("two")).unwrap();
    assert_eq!(
        root.detail(&key, Limits::default()),
        Err(StorageError::ChangedIdentity)
    );
}

#[test]
fn detail_overflow_is_an_explicit_error_and_not_a_partial_success() {
    let (_temp, chat, root) = fixture();
    fs::write(chat.join("one.jsonl"), content("one") + &" ".repeat(4096)).unwrap();
    let key = root
        .scan(ScanLimits::default())
        .unwrap()
        .sessions
        .remove(0)
        .file;
    let limits = Limits {
        max_bytes: 256,
        ..Limits::default()
    };
    assert_eq!(
        root.detail(&key, limits),
        Err(StorageError::Parse(ParseError::TooLarge))
    );
}

#[test]
fn total_preview_budget_and_entry_count_report_truncation() {
    let (_temp, chat, root) = fixture();
    for index in 0..8 {
        fs::write(
            chat.join(format!("{index}.jsonl")),
            content(&index.to_string()) + &" ".repeat(2048),
        )
        .unwrap();
    }
    let scan = root
        .scan(ScanLimits {
            prefix_bytes: 100,
            total_prefix_bytes: 150,
            ..ScanLimits::default()
        })
        .unwrap();
    assert!(scan.truncated);
    assert_eq!(scan.preview_bytes_read, 150);
    assert_eq!(scan.sessions.len(), 1);
    assert_eq!(scan.skipped, 1);
    assert!(scan.sessions.iter().all(|session| session.preview_partial));
    let scan = root
        .scan(ScanLimits {
            file_entries: 2,
            ..ScanLimits::default()
        })
        .unwrap();
    assert!(scan.truncated);
    assert_eq!(scan.sessions.len(), 2);
    assert_eq!(
        root.scan(ScanLimits {
            file_entries: 0,
            ..ScanLimits::default()
        })
        .unwrap_err(),
        StorageError::InvalidLimits
    );
}

#[test]
fn empty_missing_malformed_and_non_file_entries_are_distinguished() {
    let empty = tempfile::tempdir().unwrap();
    let root = SessionRoot::open(empty.path()).unwrap();
    assert!(
        root.scan(ScanLimits::default())
            .unwrap()
            .sessions
            .is_empty()
    );
    let (_temp, chat, root) = fixture();
    fs::write(chat.join("empty.jsonl"), "").unwrap();
    fs::write(chat.join("bad.jsonl"), "invalid JSON\n").unwrap();
    fs::create_dir(chat.join("directory.jsonl")).unwrap();
    let scan = root.scan(ScanLimits::default()).unwrap();
    assert!(scan.sessions.is_empty());
    assert_eq!(scan.skipped, 2);
}

#[test]
fn an_initial_record_beyond_the_preview_limit_never_issues_an_unbound_key() {
    let (_temp, chat, root) = fixture();
    let path = chat.join("same-path.jsonl");
    let original = serde_json::json!({"kind":0,"v":{"sessionId":"original","customTitle":"x".repeat(40 * 1024)}}).to_string() + "\n";
    fs::write(&path, &original).unwrap();
    let scan = root.scan(ScanLimits::default()).unwrap();
    assert!(scan.sessions.is_empty());
    assert_eq!(scan.skipped, 1);
    assert_eq!(fs::read_to_string(&path).unwrap(), original);
    fs::write(&path, content("replacement")).unwrap();
    let fresh = root.scan(ScanLimits::default()).unwrap();
    assert_eq!(fresh.sessions.len(), 1);
    assert_eq!(
        root.detail(&fresh.sessions[0].file, Limits::default())
            .unwrap()
            .session_id,
        "replacement"
    );
}

#[cfg(windows)]
fn link_directory(target: &Path, link: &Path) {
    use std::os::windows::process::CommandExt;
    let output = std::process::Command::new("cmd.exe")
        .args(["/D", "/C", "mklink", "/J"])
        .arg(link.to_str().unwrap().replace('/', "\\"))
        .arg(target.to_str().unwrap().replace('/', "\\"))
        .creation_flags(0x0800_0000)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "Owned test junction could not be created: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(unix)]
fn link_directory(target: &Path, link: &Path) {
    std::os::unix::fs::symlink(target, link).unwrap();
}

#[test]
fn directory_link_cannot_escape_the_selected_root() {
    let (temp, _chat, root) = fixture();
    let outside = tempfile::tempdir().unwrap();
    let external_chat = outside.path().join("chatSessions");
    fs::create_dir(&external_chat).unwrap();
    fs::write(
        external_chat.join("secret.jsonl"),
        content("outside-identity"),
    )
    .unwrap();
    let link = temp.path().join("workspaceStorage/outside-link");
    link_directory(outside.path(), &link);
    let scan = root.scan(ScanLimits::default()).unwrap();
    assert!(scan.sessions.is_empty());
    assert!(scan.skipped > 0);
    #[cfg(windows)]
    fs::remove_dir(&link).unwrap();
    #[cfg(unix)]
    fs::remove_file(&link).unwrap();
    assert!(external_chat.join("secret.jsonl").exists());
}

#[test]
fn replacing_a_scanned_directory_with_an_external_link_is_rejected_at_detail_open() {
    let (_temp, chat, root) = fixture();
    fs::write(chat.join("one.jsonl"), content("one")).unwrap();
    let key = root
        .scan(ScanLimits::default())
        .unwrap()
        .sessions
        .remove(0)
        .file;
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("one.jsonl"), content("outside")).unwrap();
    fs::remove_file(chat.join("one.jsonl")).unwrap();
    fs::remove_dir(&chat).unwrap();
    link_directory(outside.path(), &chat);
    assert_eq!(
        root.detail(&key, Limits::default()),
        Err(StorageError::ReadFailed)
    );
    #[cfg(windows)]
    fs::remove_dir(&chat).unwrap();
    #[cfg(unix)]
    fs::remove_file(&chat).unwrap();
    assert_eq!(
        fs::read_to_string(outside.path().join("one.jsonl")).unwrap(),
        content("outside")
    );
}

#[cfg(unix)]
#[test]
fn replacing_a_scanned_file_with_an_external_symlink_is_rejected() {
    let (_temp, chat, root) = fixture();
    let outside = tempfile::tempdir().unwrap();
    let file = chat.join("one.jsonl");
    fs::write(&file, content("one")).unwrap();
    let key = root
        .scan(ScanLimits::default())
        .unwrap()
        .sessions
        .remove(0)
        .file;
    let external = outside.path().join("secret.jsonl");
    fs::write(&external, content("outside")).unwrap();
    fs::remove_file(&file).unwrap();
    std::os::unix::fs::symlink(external, file).unwrap();
    assert_eq!(
        root.detail(&key, Limits::default()),
        Err(StorageError::ReadFailed)
    );
}

#[cfg(unix)]
#[test]
fn a_fifo_replacing_a_session_does_not_block_opening() {
    let (_temp, chat, root) = fixture();
    let file = chat.join("one.jsonl");
    fs::write(&file, content("one")).unwrap();
    let key = root
        .scan(ScanLimits::default())
        .unwrap()
        .sessions
        .remove(0)
        .file;
    fs::remove_file(&file).unwrap();
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&file)
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(
        root.detail(&key, Limits::default()),
        Err(StorageError::NotRegularFile)
    );
}
