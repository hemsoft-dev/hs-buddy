use super::*;

#[test]
fn cleanup_failure_remains_visible_alongside_a_cancelled_primary_result() {
    assert_eq!(
        preparation_result(Err(InstallError::Cancelled), false),
        Err(InstallError::CleanupFailed { cancelled: true })
    );
    assert_eq!(
        preparation_result(Err(InstallError::Unavailable), false),
        Err(InstallError::CleanupFailed { cancelled: false })
    );
    assert_eq!(
        preparation_result(Err(InstallError::Cancelled), true),
        Err(InstallError::Cancelled)
    );
    assert_eq!(preparation_result(Ok(vec![1]), true), Ok(vec![1]));
}

#[test]
fn cancelled_preparation_cannot_launch_or_create_directories() {
    let root = tempfile::tempdir().unwrap();
    let absent = root.path().join("must-remain-absent");
    assert!(matches!(
        prepare_blocking(
            Path::new("not-an-executable"),
            &absent,
            &AtomicBool::new(true)
        ),
        Err(InstallError::Cancelled)
    ));
    assert!(!absent.exists());
}

fn fixture(root: &Path) -> Receipt {
    std::fs::write(root.join(PROGRAM), b"synthetic wrapper").unwrap();
    std::fs::write(root.join("runtime.node"), b"synthetic native runtime").unwrap();
    Receipt {
        version: RUNTIME_VERSION.into(),
        program: root.join(PROGRAM),
        wrapper_sha256: hash_file(&root.join(PROGRAM)).unwrap(),
        runtime_sha256: hash_file(&root.join("runtime.node")).unwrap(),
    }
}

#[test]
fn only_matching_runtime_version_and_both_file_hashes_are_accepted() {
    let root = tempfile::tempdir().unwrap();
    assert!(verify_receipt(root.path(), fixture(root.path())).is_ok());
    let mut receipt = fixture(root.path());
    receipt.version = "other".into();
    assert!(verify_receipt(root.path(), receipt).is_err());
    let mut receipt = fixture(root.path());
    receipt.wrapper_sha256 = "changed".into();
    assert!(verify_receipt(root.path(), receipt).is_err());
    let mut receipt = fixture(root.path());
    receipt.runtime_sha256 = "changed".into();
    assert!(verify_receipt(root.path(), receipt).is_err());
    let receipt = fixture(root.path());
    std::fs::write(root.path().join("runtime.node"), b"tampered").unwrap();
    assert!(verify_receipt(root.path(), receipt).is_err());
}

#[test]
fn a_receipt_cannot_select_a_program_outside_its_owned_root() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    assert!(verify_receipt(root.path(), fixture(outside.path())).is_err());
    assert!(hash_file(root.path().join("missing").as_path()).is_err());
    std::fs::write(root.path().join(PROGRAM), b"").unwrap();
    assert!(hash_file(&root.path().join(PROGRAM)).is_err());
}

#[test]
fn runtime_location_rejects_stale_duplicate_and_oversize_markers() {
    let root = tempfile::tempdir().unwrap();
    let embedded = root.path().join("embedded");
    std::fs::create_dir(&embedded).unwrap();
    fixture(&embedded);
    let marker = embedded.join(".copilot-runtime-version");
    std::fs::write(&marker, RUNTIME_VERSION).unwrap();
    assert_eq!(locate_runtime(root.path()).unwrap(), embedded.join(PROGRAM));
    std::fs::write(&marker, "stale").unwrap();
    assert!(locate_runtime(root.path()).is_err());
    std::fs::write(&marker, "x".repeat(65)).unwrap();
    assert!(locate_runtime(root.path()).is_err());
    std::fs::write(&marker, RUNTIME_VERSION).unwrap();
    let nested = embedded.join("version");
    std::fs::create_dir(&nested).unwrap();
    fixture(&nested);
    std::fs::write(nested.join(".copilot-runtime-version"), RUNTIME_VERSION).unwrap();
    assert!(locate_runtime(root.path()).is_err());
}

#[test]
fn helper_arguments_preserve_unicode_as_one_argument_and_use_owned_homes() {
    let root = tempfile::tempdir().unwrap();
    let owned = root.path().join("runtime with spaces 界");
    let command = helper_command(&std::env::current_exe().unwrap(), &owned).unwrap();
    let args: Vec<_> = command.get_args().collect();
    assert_eq!(args[0], "--prepare-copilot-runtime");
    assert_eq!(Path::new(args[1]), owned.canonicalize().unwrap());
    let envs: std::collections::BTreeMap<_, _> = command.get_envs().collect();
    for key in [
        "HOME",
        "USERPROFILE",
        "APPDATA",
        "LOCALAPPDATA",
        "GH_CONFIG_DIR",
    ] {
        let value = envs[std::ffi::OsStr::new(key)].unwrap();
        assert!(Path::new(value).starts_with(owned.canonicalize().unwrap()));
    }
    assert!(
        helper_command(
            &std::env::current_exe().unwrap(),
            Path::new("relative-private")
        )
        .is_err()
    );
    for name in [
        "GH_TOKEN",
        "github_token",
        "COPILOT_CLI_PATH",
        "OPENAI_API_KEY",
        "OTEL_EXPORTER_OTLP_HEADERS",
        "NODE_OPTIONS",
        "RUST_LOG",
    ] {
        assert!(sensitive_name(std::ffi::OsStr::new(name)));
    }
    assert!(!sensitive_name(std::ffi::OsStr::new("PATH")));
}

#[test]
fn dropping_preparation_cancels_its_owned_blocking_work() {
    let cancelled = Arc::new(AtomicBool::new(false));
    {
        let _guard = CancelOnDrop(cancelled.clone());
        assert!(!cancelled.load(Ordering::Acquire));
    }
    assert!(cancelled.load(Ordering::Acquire));
}

#[cfg(unix)]
#[test]
fn a_native_library_symlink_cannot_escape_the_owned_root() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let mut receipt = fixture(root.path());
    std::fs::write(outside.path().join("outside"), b"outside").unwrap();
    std::fs::remove_file(root.path().join("runtime.node")).unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("outside"),
        root.path().join("runtime.node"),
    )
    .unwrap();
    receipt.runtime_sha256 = hash_file(&outside.path().join("outside")).unwrap();
    assert!(verify_receipt(root.path(), receipt).is_err());
}
