//! The loop scripts' repository checks (`scripts/ralph-loops/lib/repository.ps1`), run in
//! PowerShell 7 against real Git repositories: `origin` must be a GitHub repository, a
//! reused Ralph PR worktree must be clean and fast-forward, and a conflicting pull request
//! is rebased onto its own base branch.
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::scripts::BUNDLED;

const IDENTITY: [(&str, &str); 4] = [
    ("GIT_AUTHOR_NAME", "Buddy Test"),
    ("GIT_AUTHOR_EMAIL", "test@example.invalid"),
    ("GIT_COMMITTER_NAME", "Buddy Test"),
    ("GIT_COMMITTER_EMAIL", "test@example.invalid"),
];

fn git(directory: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(directory)
        .envs(IDENTITY)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {arguments:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn commit(directory: &Path, file: &str, text: &str) -> String {
    std::fs::write(directory.join(file), text).unwrap();
    git(directory, &["add", file]);
    git(directory, &["commit", "-q", "-m", file]);
    git(directory, &["rev-parse", "HEAD"])
}

/// Runs `body` in `directory` with lib/repository.ps1 loaded; its output lines, trimmed.
fn pwsh(directory: &Path, body: &str, environment: &[(&str, OsString)]) -> Vec<String> {
    let pwsh = super::launch::locate("pwsh")
        .expect("PowerShell 7 (pwsh) is required to test the Ralph scripts");
    let library = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../scripts/ralph-loops/lib/repository.ps1");
    let folder = tempfile::tempdir().unwrap();
    let driver = folder.path().join("driver.ps1");
    std::fs::write(&driver, format!(". $env:RALPH_TEST_LIBRARY\n{body}\n")).unwrap();
    let output = Command::new(pwsh)
        .args(["-NoProfile", "-NonInteractive", "-File"])
        .arg(&driver)
        .current_dir(directory)
        .env("RALPH_TEST_LIBRARY", library)
        .envs(IDENTITY)
        .envs(environment.iter().map(|(name, value)| (*name, value)))
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "pwsh failed: {stdout}{}",
        String::from_utf8_lossy(&output.stderr)
    );
    stdout.lines().map(|line| line.trim().to_owned()).collect()
}

/// A bare `remote.git` with `main`, and the `seed` checkout that pushes to it.
fn remote(root: &Path) -> (PathBuf, PathBuf) {
    let remote = root.join("remote.git");
    let seed = root.join("seed");
    git(
        root,
        &[
            "init",
            "-q",
            "--bare",
            "-b",
            "main",
            remote.to_str().unwrap(),
        ],
    );
    git(root, &["init", "-q", "-b", "main", seed.to_str().unwrap()]);
    commit(&seed, "base.txt", "base\n");
    git(
        &seed,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(&seed, &["push", "-q", "origin", "main"]);
    (remote, seed)
}

#[test]
fn scripts_resolve_only_an_origin_on_github_and_confirm_it_with_gh() {
    let root = tempfile::tempdir().unwrap();
    let repository = root.path().join("repo");
    std::fs::create_dir(&repository).unwrap();
    git(&repository, &["init", "-q"]);
    // A stand-in gh that knows one repository, by the name GitHub gives it.
    let tools = root.path().join("tools");
    std::fs::create_dir(&tools).unwrap();
    if cfg!(windows) {
        std::fs::write(
            tools.join("gh.cmd"),
            "@echo off\r\nif \"%3\"==\"github.com/acme/project\" (echo Acme/Project& exit /b 0)\r\nexit /b 1\r\n",
        )
        .unwrap();
    } else {
        let stub = tools.join("gh");
        std::fs::write(
            &stub,
            "#!/bin/sh\n[ \"$3\" = github.com/acme/project ] && { echo Acme/Project; exit 0; }\nexit 1\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    let path = std::env::join_paths(std::iter::once(tools).chain(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    )))
    .unwrap();
    let github = [
        "https://github.com/acme/project.git",
        "git@github.com:acme/project.git",
        "ssh://git@ssh.github.com:443/acme/project",
        "git@github-work:acme/project.git",
    ];
    let elsewhere = [
        "/srv/acme/project.git",
        "file:///srv/acme/project.git",
        "C:/srv/acme/project.git",
        "../acme/project.git",
        "https://gitlab.com/acme/project.git",
        "git@gitlab-work:acme/project.git",
        "https://github.com.evil.example/acme/project.git",
    ];
    let urls = github
        .iter()
        .chain(&["https://github.com/acme/missing"])
        .chain(&elsewhere)
        .copied()
        .collect::<Vec<_>>();
    let lines = pwsh(
        &repository,
        r#"
# A stand-in for ssh -G: only github-work is an alias for GitHub.
$resolve = { param($HostName) if ($HostName -eq 'github-work') { 'github.com' } }
function Write-Origin([string]$Label) {
    $origin = Get-RalphOriginRemote -ResolveSshHost $resolve
    if ($origin.Error) { "$Label => $($origin.Error)" }
    else { "$Label => $($origin.Slug) => $(Confirm-RalphGitHubRepo -Slug $origin.Slug)" }
}
Write-Origin 'none'
git remote add origin placeholder
foreach ($url in $env:RALPH_TEST_URLS -split ';') {
    git remote set-url origin $url
    Write-Origin $url
}
"#,
        &[("PATH", path), ("RALPH_TEST_URLS", urls.join(";").into())],
    );
    let refused = "origin is not a GitHub repository (git remote get-url origin). Ralph only works on GitHub repositories.";
    let mut expected = vec![
        "none => This repository has no origin remote. Ralph works on the GitHub repository origin names."
            .to_owned(),
    ];
    expected.extend(
        github
            .iter()
            .map(|url| format!("{url} => acme/project => Acme/Project")),
    );
    // On GitHub by its URL, but gh cannot resolve it.
    expected.push("https://github.com/acme/missing => acme/missing =>".into());
    expected.extend(elsewhere.iter().map(|url| format!("{url} => {refused}")));
    assert_eq!(lines, expected);
}

#[test]
fn ralph_pr_reuses_only_a_clean_tracking_worktree_that_fast_forwards() {
    let root = tempfile::tempdir().unwrap();
    let (remote, seed) = remote(root.path());
    git(&seed, &["checkout", "-q", "-b", "topic"]);
    commit(&seed, "topic.txt", "one\n");
    git(&seed, &["push", "-q", "origin", "topic"]);
    let clone = root.path().join("clone");
    git(
        root.path(),
        &[
            "clone",
            "-q",
            remote.to_str().unwrap(),
            clone.to_str().unwrap(),
        ],
    );
    let worktree = root.path().join("clone.worktrees").join("topic");
    git(
        &clone,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "topic",
            worktree.to_str().unwrap(),
            "origin/topic",
        ],
    );
    git(
        &worktree,
        &["branch", "-q", "--set-upstream-to", "origin/topic"],
    );
    let reuse = || {
        pwsh(
            &clone,
            "$reason = Test-RalphReusedWorktree -Path $env:RALPH_TEST_WORKTREE -Branch topic\nif ($reason) { $reason } else { 'ready' }",
            &[("RALPH_TEST_WORKTREE", worktree.clone().into_os_string())],
        )
        .join("\n")
    };
    // Behind the pull request: it fast-forwards to the current head.
    let head = commit(&seed, "topic.txt", "two\n");
    git(&seed, &["push", "-q", "origin", "topic"]);
    assert_eq!(reuse(), "ready");
    assert_eq!(git(&worktree, &["rev-parse", "HEAD"]), head);
    // Untracked or changed files, as an interrupted run leaves them.
    std::fs::write(worktree.join("notes.txt"), "left behind\n").unwrap();
    assert_eq!(
        reuse(),
        "it has uncommitted changes. Commit, stash or discard them first."
    );
    std::fs::remove_file(worktree.join("notes.txt")).unwrap();
    std::fs::write(worktree.join("topic.txt"), "edited\n").unwrap();
    assert!(reuse().starts_with("it has uncommitted changes."));
    git(&worktree, &["checkout", "-q", "--", "topic.txt"]);
    // No upstream, or another branch's.
    git(&worktree, &["branch", "-q", "--unset-upstream"]);
    assert!(
        reuse().starts_with("branch 'topic' has no upstream."),
        "{}",
        reuse()
    );
    git(
        &worktree,
        &["branch", "-q", "--set-upstream-to", "origin/main"],
    );
    assert_eq!(
        reuse(),
        "branch 'topic' tracks origin/main, not origin/topic."
    );
    git(
        &worktree,
        &["branch", "-q", "--set-upstream-to", "origin/topic"],
    );
    assert_eq!(reuse(), "ready");
    // Diverged from the pull request: never used.
    let local = commit(&worktree, "local.txt", "local\n");
    commit(&seed, "topic.txt", "three\n");
    git(&seed, &["push", "-q", "origin", "topic"]);
    assert!(
        reuse().starts_with("it could not fast-forward to origin/topic (git pull --ff-only):"),
        "{}",
        reuse()
    );
    assert_eq!(git(&worktree, &["rev-parse", "HEAD"]), local);
}

#[test]
fn ralph_pr_rebases_conflicts_onto_the_pull_requests_own_base() {
    let root = tempfile::tempdir().unwrap();
    let (remote, seed) = remote(root.path());
    let base = git(&seed, &["rev-parse", "HEAD"]);
    git(&seed, &["checkout", "-q", "-b", "release/1.0"]);
    let release = commit(&seed, "release.txt", "release\n");
    git(&seed, &["push", "-q", "origin", "release/1.0"]);
    git(&seed, &["checkout", "-q", "main"]);
    commit(&seed, "main.txt", "main only\n");
    git(&seed, &["push", "-q", "origin", "main"]);
    git(&seed, &["checkout", "-q", "-b", "topic", &base]);
    commit(&seed, "topic.txt", "topic\n");
    git(&seed, &["push", "-q", "origin", "topic"]);
    // A clone of the PR branch alone has no origin/release/1.0 until the base is fetched.
    let clone = root.path().join("clone");
    git(
        root.path(),
        &[
            "clone",
            "-q",
            "--single-branch",
            "-b",
            "topic",
            remote.to_str().unwrap(),
            clone.to_str().unwrap(),
        ],
    );
    let rebase = |base: &str| {
        pwsh(
            &clone,
            "$reason = Invoke-RalphRebaseOnto -Base $env:RALPH_TEST_BASE\nif ($reason) { $reason } else { 'rebased' }",
            &[("RALPH_TEST_BASE", base.into())],
        )
        .join("\n")
    };
    assert_eq!(rebase("release/1.0"), "rebased");
    assert_eq!(git(&clone, &["rev-parse", "HEAD~1"]), release);
    assert!(clone.join("release.txt").exists());
    assert!(!clone.join("main.txt").exists(), "rebased onto main");
    // A conflict is aborted, leaving the branch as it was.
    let head = git(&clone, &["rev-parse", "HEAD"]);
    git(&seed, &["checkout", "-q", "release/1.0"]);
    commit(&seed, "topic.txt", "release's own\n");
    git(&seed, &["push", "-q", "origin", "release/1.0"]);
    assert!(rebase("release/1.0").starts_with("git rebase origin/release/1.0 failed:"));
    assert_eq!(git(&clone, &["rev-parse", "HEAD"]), head);
    assert_eq!(git(&clone, &["status", "--porcelain"]), "");
    assert!(rebase("gone").starts_with("git fetch origin gone failed:"));
}

#[test]
fn loop_scripts_use_the_shared_checks() {
    let text = |name: &str| BUNDLED.iter().find(|(file, _)| *file == name).unwrap().1;
    for name in ["ralph.ps1", "ralph-pr.ps1", "ralph-issues.ps1"] {
        let script = text(name);
        assert!(script.contains(". (Join-Path $PSScriptRoot 'lib' 'repository.ps1')"));
        // origin is checked first, then gh confirms it once the account is matched.
        let origin = script.find("Get-RalphOriginRemote").unwrap();
        let confirm = script
            .find("Confirm-RalphGitHubRepo -Slug $originRepo.Slug")
            .unwrap();
        assert!(origin < confirm, "{name}");
        // No slug read from an arbitrary origin URL.
        assert!(!script.contains("remote get-url origin"), "{name}");
    }
    let pr = text("ralph-pr.ps1");
    let agent = pr.find("& $cmdExe @cmdTail").unwrap();
    assert!(
        pr.find("Test-RalphReusedWorktree -Path $worktreeDir -Branch $prBranch")
            .unwrap()
            < agent
    );
    assert!(pr.contains("--json headRefName,baseRefName,state,isCrossRepository"));
    assert!(pr.contains("$rebaseError = Invoke-RalphRebaseOnto -Base $script:PRBase"));
    assert!(!pr.contains("origin/main"));
}
