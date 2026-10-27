use super::*;

fn found(root: &Path, slug: &str) -> project::Found {
    std::fs::create_dir_all(root).unwrap();
    project::Found {
        git_root: root.to_path_buf(),
        slug: slug.into(),
        default_branch: "main".into(),
    }
}

fn store() -> (tempfile::TempDir, Store) {
    let temporary = tempfile::tempdir().unwrap();
    let store = Store::at(temporary.path().join("native-crew")).unwrap();
    (temporary, store)
}

fn session(store: &Store, project: &Project) -> Session {
    let (id, _) = store.prepare_session().unwrap();
    store
        .start_session(&project.id, &id, "octo-cat", "gpt-4.1")
        .unwrap()
}

#[test]
fn projects_are_registered_once_per_root_and_sorted() {
    let (temporary, store) = store();
    let zeta = store
        .add(found(&temporary.path().join("z"), "o/zeta"))
        .unwrap();
    let alpha = store
        .add(found(&temporary.path().join("a"), "o/Alpha"))
        .unwrap();
    let again = store
        .add(found(&temporary.path().join("z"), "o/zeta"))
        .unwrap();
    assert_eq!(again.id, zeta.id);
    assert!(again.last_opened_at >= zeta.last_opened_at);
    let names: Vec<_> = store
        .projects()
        .unwrap()
        .into_iter()
        .map(|project| project.display_name)
        .collect();
    assert_eq!(names, ["Alpha", "zeta"]);
    assert_eq!(alpha.github_slug, "o/Alpha");
    assert_eq!(alpha.local_path, temporary.path().join("a"));
}

#[test]
fn removing_a_project_drops_its_conversation_and_snapshot_only() {
    let (temporary, store) = store();
    let one = store
        .add(found(&temporary.path().join("one"), "o/one"))
        .unwrap();
    let two = store
        .add(found(&temporary.path().join("two"), "o/two"))
        .unwrap();
    let first = session(&store, &one);
    let second = session(&store, &two);
    assert!(store.snapshot_folder(&first.id).is_dir());

    assert!(store.remove(&one.id).unwrap().done);
    assert!(!store.remove(&one.id).unwrap().done);
    assert!(!store.snapshot_folder(&first.id).exists());
    assert!(store.snapshot_folder(&second.id).is_dir());
    assert_eq!(store.session(&one.id).unwrap(), None);
    assert_eq!(store.session(&two.id).unwrap().unwrap().id, second.id);
    // The repository folder itself is untouched.
    assert!(temporary.path().join("one").is_dir());
}

#[test]
fn a_project_reuses_its_conversation() {
    let (temporary, store) = store();
    let project = store
        .add(found(&temporary.path().join("p"), "o/p"))
        .unwrap();
    let first = session(&store, &project);
    let (late, folder) = store.prepare_session().unwrap();
    let reused = store
        .start_session(&project.id, &late, "someone-else", "other")
        .unwrap();
    assert_eq!(reused, first);
    assert!(!folder.exists(), "the losing snapshot is discarded");

    let (id, folder) = store.prepare_session().unwrap();
    assert!(store.start_session(&project.id, &id, "-bad", "m").is_err());
    assert!(store.start_session(&project.id, &id, "ok", " ").is_err());
    assert!(store.start_session("crew-gone", &id, "ok", "m").is_err());
    assert!(!folder.exists());
}

#[test]
fn a_turn_records_one_question_and_one_answer() {
    let (temporary, store) = store();
    let project = store
        .add(found(&temporary.path().join("p"), "o/p"))
        .unwrap();
    let started = session(&store, &project);

    assert_eq!(
        store.begin_turn(&started.id, "r1", "   ").unwrap_err(),
        TurnError::Invalid("Enter a message.")
    );
    let (active, guard) = store.begin_turn(&started.id, "r1", " Hello ").unwrap();
    assert_eq!(active.status, Status::Active);
    assert_eq!(active.conversation_history.len(), 1);
    assert_eq!(active.conversation_history[0].content, "Hello");
    assert!(store.turn_running(&started.id));
    // A second send (another click or window) cannot start while this reply runs.
    assert_eq!(
        store.begin_turn(&started.id, "r2", "again").err(),
        Some(TurnError::Busy)
    );
    assert_eq!(
        store.clear(&project.id).unwrap_err(),
        "Stop the reply in progress before clearing the conversation."
    );
    // Another window's Remove waits for the reply too.
    let other = Store::at(store.root().to_path_buf()).unwrap();
    assert_eq!(
        other.remove(&project.id).unwrap_err(),
        "Stop the reply in progress before removing the project."
    );

    // A reply for another request is ignored.
    let stray = Reply::Complete("stray".into());
    assert_eq!(store.finish_turn(&started.id, "r2", stray).unwrap(), None);
    let done = store
        .finish_turn(&started.id, "r1", Reply::Complete("Hi".into()))
        .unwrap()
        .unwrap();
    drop(guard);
    assert_eq!(done.status, Status::Idle);
    assert_eq!(done.pending, None);
    let roles: Vec<_> = done.conversation_history.iter().map(|m| m.role).collect();
    assert_eq!(roles, [Role::User, Role::Assistant]);
    // Delivering the same reply again changes nothing.
    let again = Reply::Complete("Hi".into());
    assert_eq!(store.finish_turn(&started.id, "r1", again).unwrap(), None);
    assert_eq!(
        store
            .session(&project.id)
            .unwrap()
            .unwrap()
            .conversation_history
            .len(),
        2
    );
    assert!(!store.turn_running(&started.id));
    assert!(store.project(&project.id).unwrap().unwrap().last_active_at >= project.last_active_at);
}

#[test]
fn failures_and_cancellations_keep_what_arrived() {
    let (temporary, store) = store();
    let project = store
        .add(found(&temporary.path().join("p"), "o/p"))
        .unwrap();
    let started = session(&store, &project);
    let (_, guard) = store.begin_turn(&started.id, "r1", "one").unwrap();
    let failed = Reply::Failed {
        partial: "Half".into(),
        reason: "The assistant service is unavailable.".into(),
    };
    let after = store
        .finish_turn(&started.id, "r1", failed)
        .unwrap()
        .unwrap();
    drop(guard);
    let last = after.conversation_history.last().unwrap();
    assert!(last.error && last.partial);
    assert_eq!(
        last.content,
        "Half\n\nThe assistant service is unavailable."
    );
    assert_eq!(after.status, Status::Error);

    let (_, guard) = store.begin_turn(&started.id, "r2", "two").unwrap();
    let cut = Reply::Partial("Par".into());
    let after = store.finish_turn(&started.id, "r2", cut).unwrap().unwrap();
    drop(guard);
    let last = after.conversation_history.last().unwrap();
    assert!(last.partial && !last.error);
    assert_eq!(after.status, Status::Idle);
}

#[test]
fn a_cleared_conversation_ignores_its_late_reply() {
    let (temporary, store) = store();
    let project = store
        .add(found(&temporary.path().join("p"), "o/p"))
        .unwrap();
    let started = session(&store, &project);
    let (_, guard) = store.begin_turn(&started.id, "r1", "one").unwrap();
    drop(guard);
    assert!(store.clear(&project.id).unwrap().done);
    assert!(!store.snapshot_folder(&started.id).exists());
    let late = Reply::Complete("late".into());
    assert_eq!(store.finish_turn(&started.id, "r1", late).unwrap(), None);
    assert_eq!(store.session(&project.id).unwrap(), None);
    assert!(!store.clear(&project.id).unwrap().done);
    // A new conversation starts empty.
    let next = session(&store, &project);
    assert_ne!(next.id, started.id);
    assert!(next.conversation_history.is_empty());
}

#[test]
fn replies_interrupted_by_a_closed_buddy_are_recovered() {
    let (temporary, store) = store();
    let project = store
        .add(found(&temporary.path().join("p"), "o/p"))
        .unwrap();
    let other = store
        .add(found(&temporary.path().join("q"), "o/q"))
        .unwrap();
    let orphan = session(&store, &project);
    let live = session(&store, &other);
    let (_, closed) = store.begin_turn(&orphan.id, "r1", "lost").unwrap();
    drop(closed);
    let (_, running) = store.begin_turn(&live.id, "r2", "running").unwrap();

    // Another window opening the same store.
    let reopened = Store::at(store.root().to_path_buf()).unwrap();
    assert_eq!(reopened.recover().unwrap(), 1);
    let recovered = reopened.session(&project.id).unwrap().unwrap();
    assert_eq!(recovered.status, Status::Error);
    assert_eq!(recovered.pending, None);
    assert_eq!(
        recovered.conversation_history.last().unwrap().content,
        "Buddy closed before this reply finished."
    );
    let still = reopened.session(&other.id).unwrap().unwrap();
    assert!(
        still.pending.is_some(),
        "a reply another window runs is left alone"
    );
    drop(running);
    assert_eq!(reopened.recover().unwrap(), 1);
    // The conversation can continue.
    let (_, guard) = reopened.begin_turn(&orphan.id, "r3", "again").unwrap();
    drop(guard);
}

#[test]
fn writes_from_two_windows_are_both_kept() {
    let (temporary, store) = store();
    let other = Store::at(store.root().to_path_buf()).unwrap();
    store
        .add(found(&temporary.path().join("a"), "o/a"))
        .unwrap();
    other
        .add(found(&temporary.path().join("b"), "o/b"))
        .unwrap();
    assert_eq!(store.projects().unwrap().len(), 2);
}

#[test]
fn a_damaged_state_file_is_reported_not_replaced() {
    let (_temporary, store) = store();
    let path = store.root().join("state.json");
    std::fs::write(&path, b"{not json").unwrap();
    let error = store.projects().unwrap_err();
    assert!(
        error.starts_with("The Crew state file is damaged."),
        "{error}"
    );
    assert!(store.add(found(&store.root().join("x"), "o/x")).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"{not json");
}

#[test]
fn only_issued_ids_name_folders() {
    let (_temporary, store) = store();
    for id in ["", "../x", "a/b", "a\\b", "C:"] {
        assert!(!valid_id(id), "{id}");
        assert!(!store.turn_running(id));
        assert_eq!(
            store.begin_turn(id, "r", "x").err(),
            Some(TurnError::NoSession)
        );
    }
    assert!(valid_id("session-0123abc"));
}

#[test]
fn state_uses_electrons_field_names() {
    let (temporary, store) = store();
    let project = store
        .add(found(&temporary.path().join("p"), "o/p"))
        .unwrap();
    session(&store, &project);
    let text = std::fs::read_to_string(store.root().join("state.json")).unwrap();
    for field in [
        "displayName",
        "localPath",
        "gitRoot",
        "githubSlug",
        "defaultBranch",
        "lastOpenedAt",
        "projectId",
        "conversationHistory",
        "\"status\": \"idle\"",
    ] {
        assert!(text.contains(field), "{field}");
    }
}

#[test]
fn nothing_is_written_while_another_window_holds_the_state_lock() {
    let (temporary, store) = store();
    let held = ConfigLock::acquire(&store.root().join("state.json"), LOCK_TIMEOUT)
        .unwrap()
        .unwrap();
    let error = store
        .add(found(&temporary.path().join("a"), "o/a"))
        .unwrap_err();
    assert_eq!(
        error,
        "Another Buddy window is saving the Crew state. Try again."
    );
    drop(held);
    assert!(store.add(found(&temporary.path().join("a"), "o/a")).is_ok());
}

#[test]
fn snapshots_no_conversation_names_are_swept_at_start() {
    let (temporary, store) = store();
    let project = store
        .add(found(&temporary.path().join("p"), "o/p"))
        .unwrap();
    let kept = session(&store, &project);
    let orphan = store.snapshot_folder("session-orphan");
    std::fs::create_dir_all(orphan.join("blobs")).unwrap();
    std::fs::write(orphan.join("blobs").join("x"), b"copy").unwrap();
    std::fs::write(store.root().join("turns").join("session-orphan.lock"), b"").unwrap();
    store.recover().unwrap();
    assert!(!orphan.exists());
    assert!(
        !store
            .root()
            .join("turns")
            .join("session-orphan.lock")
            .exists()
    );
    assert!(store.snapshot_folder(&kept.id).is_dir());
}

/// A committed repository whose `origin` names GitHub, for tests that capture snapshots.
fn git_repository(root: &Path) -> Option<(PathBuf, PathBuf)> {
    let git = project::git().ok()?;
    std::fs::create_dir_all(root).unwrap();
    let run = |arguments: &[&str]| {
        let mut all = vec!["-c", "user.name=t", "-c", "user.email=t@example.invalid"];
        all.extend_from_slice(arguments);
        assert!(
            project::output(&git, Some(root), &all).is_some(),
            "{arguments:?}"
        );
    };
    run(&["init", "-q"]);
    std::fs::write(root.join("a.txt"), "a\n").unwrap();
    run(&["add", "-A"]);
    run(&["-c", "commit.gpgsign=false", "commit", "-q", "-m", "c"]);
    Some((git, root.to_path_buf()))
}

fn has_ref(git: &Path, root: &Path, session: &str) -> bool {
    let name = format!("refs/buddy/crew/{session}");
    project::output(
        git,
        Some(root),
        ["rev-parse", "--verify", "-q", name.as_str()],
    )
    .is_some()
}

#[test]
fn a_session_that_fails_to_register_leaves_no_snapshot_or_ref() {
    let (temporary, store) = store();
    let Some((git, root)) = git_repository(&temporary.path().join("repo")) else {
        return;
    };
    let project = store.add(found(&root, "o/repo")).unwrap();
    let (id, folder) = store.prepare_session().unwrap();
    changes::capture(&git, &root, &folder).unwrap();
    assert!(has_ref(&git, &root, &id));
    let too_long = "m".repeat(300);
    assert!(
        store
            .start_session(&project.id, &id, "octo", &too_long)
            .is_err()
    );
    assert!(!folder.exists());
    assert!(!has_ref(&git, &root, &id), "the snapshot ref is released");
}

#[test]
fn the_sweep_releases_a_leftover_snapshots_ref() {
    let (temporary, store) = store();
    let Some((git, root)) = git_repository(&temporary.path().join("repo")) else {
        return;
    };
    // A capture whose conversation was never recorded (Buddy closed in between).
    let (id, folder) = store.prepare_session().unwrap();
    changes::capture(&git, &root, &folder).unwrap();
    assert!(has_ref(&git, &root, &id));
    store.recover().unwrap();
    assert!(!folder.exists());
    assert!(!has_ref(&git, &root, &id));
}
