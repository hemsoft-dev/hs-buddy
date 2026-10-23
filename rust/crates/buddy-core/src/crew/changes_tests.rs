use super::*;

struct Repo {
    _temporary: tempfile::TempDir,
    git: PathBuf,
    root: PathBuf,
    folder: PathBuf,
}

impl Repo {
    fn new() -> Option<Self> {
        let git = super::super::project::git().ok()?;
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("project with spaces é");
        let folder = temporary.path().join("snapshot");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&folder).unwrap();
        let repo = Self {
            _temporary: temporary,
            git,
            root,
            folder,
        };
        repo.run(&["init", "-q"]);
        // Every Git call in the test, including the code under test, sees LF checkouts.
        repo.run(&["config", "core.autocrlf", "false"]);
        Some(repo)
    }

    fn run(&self, arguments: &[&str]) {
        let mut all = vec![
            "-c",
            "user.name=Crew Test",
            "-c",
            "user.email=crew@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.autocrlf=false",
        ];
        all.extend_from_slice(arguments);
        assert!(
            output(&self.git, Some(&self.root), &all).is_some(),
            "git {arguments:?}"
        );
    }

    fn write(&self, path: &str, text: &str) {
        let path = self.root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn read(&self, path: &str) -> Option<String> {
        std::fs::read_to_string(self.root.join(path)).ok()
    }

    fn commit_all(&self) {
        self.run(&["add", "-A"]);
        self.run(&["commit", "-q", "-m", "c"]);
    }

    fn capture(&self) {
        capture(&self.git, &self.root, &self.folder).unwrap();
    }

    fn scan(&self) -> Scan {
        scan(&self.git, &self.root, &self.folder).unwrap()
    }

    fn change(&self, path: &str) -> Change {
        self.scan()
            .changes
            .into_iter()
            .find(|change| change.path == path)
            .unwrap_or_else(|| panic!("{path} is not listed"))
    }

    fn listed(&self) -> Vec<(String, Kind)> {
        self.scan()
            .changes
            .into_iter()
            .map(|change| (change.path, change.kind))
            .collect()
    }

    fn undo(&self, path: &str) -> Result<(), ActionError> {
        let seen = self.change(path).seen;
        undo(&self.git, &self.root, &self.folder, path, &seen)
    }
}

#[test]
fn changes_are_measured_from_the_session_start_not_from_head() {
    let Some(repo) = Repo::new() else { return };
    repo.write("a.txt", "one\ntwo\n");
    repo.write("dir/b.txt", "bee\n");
    repo.commit_all();
    // Edits that existed before the session are the user's, not the session's.
    repo.write("a.txt", "one\nmine\n");
    repo.write("untracked.txt", "u\n");
    repo.capture();
    assert_eq!(repo.scan(), Scan::default());

    repo.write("a.txt", "one\nmine\nmore\n");
    repo.write("dir/b.txt", "bee\nsting\n");
    repo.write("new file.txt", "fresh\n");
    std::fs::remove_file(repo.root.join("untracked.txt")).unwrap();
    assert_eq!(
        repo.listed(),
        [
            ("a.txt".to_string(), Kind::Modified),
            ("dir/b.txt".into(), Kind::Modified),
            ("new file.txt".into(), Kind::Added),
            ("untracked.txt".into(), Kind::Deleted),
        ]
    );
    assert_eq!(repo.change("a.txt").lines, Some((1, 0)));
    assert_eq!(repo.change("new file.txt").lines, Some((1, 0)));
    assert_eq!(repo.change("untracked.txt").lines, Some((0, 1)));

    // Undo restores each file to the session's start, keeping the user's earlier edit.
    for path in ["a.txt", "dir/b.txt", "new file.txt", "untracked.txt"] {
        repo.undo(path).unwrap();
    }
    assert_eq!(repo.read("a.txt").unwrap(), "one\nmine\n");
    assert_eq!(repo.read("dir/b.txt").unwrap(), "bee\n");
    assert_eq!(repo.read("new file.txt"), None);
    assert_eq!(repo.read("untracked.txt").unwrap(), "u\n");
    assert_eq!(repo.scan(), Scan::default());
}

#[test]
fn keep_accepts_the_current_content_until_it_changes_again() {
    let Some(repo) = Repo::new() else { return };
    repo.write("a.txt", "a\n");
    repo.commit_all();
    repo.capture();
    repo.write("a.txt", "a\nkept\n");
    let seen = repo.change("a.txt").seen;
    keep(&repo.root, &repo.folder, "a.txt", &seen).unwrap();
    assert_eq!(repo.scan(), Scan::default());

    repo.write("a.txt", "a\nkept\nlater\n");
    assert_eq!(repo.change("a.txt").lines, Some((1, 0)));
    // Undo now returns to the kept content, not the commit.
    repo.undo("a.txt").unwrap();
    assert_eq!(repo.read("a.txt").unwrap(), "a\nkept\n");

    // Keeping a deletion accepts it too.
    std::fs::remove_file(repo.root.join("a.txt")).unwrap();
    let seen = repo.change("a.txt").seen;
    assert_eq!(seen, Seen::Absent);
    keep(&repo.root, &repo.folder, "a.txt", &seen).unwrap();
    assert_eq!(repo.scan(), Scan::default());
}

#[test]
fn an_edit_after_the_list_was_read_is_never_overwritten() {
    let Some(repo) = Repo::new() else { return };
    repo.write("a.txt", "a\n");
    repo.commit_all();
    repo.capture();
    repo.write("a.txt", "agent\n");
    let seen = repo.change("a.txt").seen;
    repo.write("a.txt", "the user's newer edit\n");
    let undone = undo(&repo.git, &repo.root, &repo.folder, "a.txt", &seen);
    assert_eq!(undone, Err(ActionError::Changed));
    assert_eq!(
        keep(&repo.root, &repo.folder, "a.txt", &seen),
        Err(ActionError::Changed)
    );
    assert_eq!(repo.read("a.txt").unwrap(), "the user's newer edit\n");
}

#[test]
fn paths_outside_the_project_are_refused() {
    let Some(repo) = Repo::new() else { return };
    repo.write("a.txt", "a\n");
    repo.commit_all();
    repo.capture();
    let outside = repo.root.parent().unwrap().join("outside.txt");
    std::fs::write(&outside, "keep me\n").unwrap();
    let seen = Seen::File(digest(b"keep me\n"));
    for path in [
        "../outside.txt",
        ".git/config",
        ".GIT/HEAD",
        "/etc/passwd",
        "dir\\..\\..\\outside.txt",
        "./a.txt",
        "",
    ] {
        let refused = undo(&repo.git, &repo.root, &repo.folder, path, &seen);
        assert!(matches!(refused, Err(ActionError::Refused(_))), "{path}");
        let refused = keep(&repo.root, &repo.folder, path, &seen);
        assert!(matches!(refused, Err(ActionError::Refused(_))), "{path}");
    }
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), "keep me\n");
}

#[cfg(unix)]
#[test]
fn a_link_out_of_the_project_is_not_followed() {
    let Some(repo) = Repo::new() else { return };
    repo.write("a.txt", "a\n");
    repo.commit_all();
    repo.capture();
    let outside = repo.root.parent().unwrap().join("elsewhere");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("f.txt"), "outside\n").unwrap();
    std::os::unix::fs::symlink(&outside, repo.root.join("link")).unwrap();
    let change = repo.change("link");
    assert_eq!(change.kind, Kind::Added);
    assert!(change.no_undo.is_some());
    let seen = Seen::File(digest(b"outside\n"));
    let refused = undo(&repo.git, &repo.root, &repo.folder, "link/f.txt", &seen);
    assert!(refused.is_err());
    assert_eq!(
        std::fs::read_to_string(outside.join("f.txt")).unwrap(),
        "outside\n"
    );
}

#[test]
fn line_endings_follow_gits_checkout_filters() {
    let Some(repo) = Repo::new() else { return };
    repo.write(".gitattributes", "*.txt text eol=crlf\n");
    repo.write("a.txt", "one\ntwo\n");
    repo.commit_all();
    // Check the file out again so the working tree holds CRLF.
    std::fs::remove_file(repo.root.join("a.txt")).unwrap();
    repo.run(&["checkout", "--", "a.txt"]);
    assert_eq!(repo.read("a.txt").unwrap(), "one\r\ntwo\r\n");
    repo.capture();
    assert_eq!(repo.scan(), Scan::default());
    repo.write("a.txt", "one\r\nTWO\r\n");
    assert_eq!(repo.change("a.txt").lines, Some((1, 1)));
    repo.undo("a.txt").unwrap();
    assert_eq!(repo.read("a.txt").unwrap(), "one\r\ntwo\r\n");
}

#[test]
fn commits_made_during_the_session_still_count() {
    let Some(repo) = Repo::new() else { return };
    repo.write("a.txt", "a\n");
    repo.commit_all();
    repo.capture();
    repo.write("a.txt", "a\nb\n");
    repo.write("c.txt", "c\n");
    repo.commit_all();
    assert_eq!(
        repo.listed(),
        [
            ("a.txt".to_string(), Kind::Modified),
            ("c.txt".into(), Kind::Added)
        ]
    );
    repo.undo("c.txt").unwrap();
    repo.undo("a.txt").unwrap();
    assert_eq!(repo.read("a.txt").unwrap(), "a\n");
    assert_eq!(repo.scan(), Scan::default());
}

#[test]
fn a_repository_without_commits_starts_from_its_files() {
    let Some(repo) = Repo::new() else { return };
    repo.write("a.txt", "a\n");
    repo.capture();
    assert_eq!(repo.scan(), Scan::default());
    repo.commit_all();
    repo.write("a.txt", "a\nb\n");
    repo.write("b.txt", "b\n");
    repo.commit_all();
    assert_eq!(
        repo.listed(),
        [
            ("a.txt".to_string(), Kind::Modified),
            ("b.txt".into(), Kind::Added)
        ]
    );
    repo.undo("a.txt").unwrap();
    assert_eq!(repo.read("a.txt").unwrap(), "a\n");
}

#[test]
fn binary_and_ignored_files() {
    let Some(repo) = Repo::new() else { return };
    repo.write(".gitignore", "build/\n");
    repo.commit_all();
    repo.capture();
    std::fs::create_dir_all(repo.root.join("build")).unwrap();
    std::fs::write(repo.root.join("build/out.bin"), b"ignored").unwrap();
    std::fs::write(repo.root.join("image.bin"), b"\0\x01\x02").unwrap();
    let listed = repo.scan().changes;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].path, "image.bin");
    assert_eq!(listed[0].lines, None);
    repo.undo("image.bin").unwrap();
    assert!(!repo.root.join("image.bin").exists());
}

#[test]
fn reported_paths_are_validated() {
    for path in [
        "a.txt",
        "dir/b c.txt",
        "é/ü.txt",
        ".github/x.yml",
        ".gitignore",
    ] {
        assert!(valid_path(path), "{path}");
    }
    for path in [
        "",
        "../a",
        "a/../../b",
        "/a",
        ".git",
        ".git/x",
        "a\\b",
        "a\nb",
        "./a",
    ] {
        assert!(!valid_path(path), "{path}");
    }
    // A drive or UNC prefix is a different volume on Windows.
    #[cfg(windows)]
    for path in ["C:/Windows/win.ini", "C:a.txt", "//server/share/a"] {
        assert!(!valid_path(path), "{path}");
    }
}
