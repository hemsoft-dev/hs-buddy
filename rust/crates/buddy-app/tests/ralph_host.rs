//! The installed binary's Ralph host runs a loop detached, records its progress and
//! result, and stops exactly the loop's own process tree. A stub script stands in for the
//! agent loop; it needs PowerShell 7, which every CI runner image provides.
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use buddy_core::ralph::{
    host::spawn_host,
    launch::{Launch, Plan},
    progress::Phase,
    store::{RunState, Store},
};

fn pwsh() -> PathBuf {
    tool(if cfg!(windows) { "pwsh.exe" } else { "pwsh" })
}

fn tool(name: &str) -> PathBuf {
    std::env::var_os("PATH")
        .and_then(|path| {
            std::env::split_paths(&path)
                .map(|directory| directory.join(name))
                .find(|path| path.is_file())
        })
        .unwrap_or_else(|| panic!("{name} is required on PATH for the Ralph host tests"))
}

const STUB: &str = r#"param([int]$Max = 1, [string]$Prompt, [int]$Code = 0, [string]$Beat, [string]$Account, [switch]$Unset, [switch]$Stale, [string]$Daemon)
Write-Host "== Iteration 1/$Max"
if ($Unset) { git branch --unset-upstream }
if ($Account) {
    $expected = "stub-token-$Account"
    Write-Host "token $($env:GH_TOKEN -eq $expected) $($env:GITHUB_TOKEN -eq $expected)"
    Write-Host "account $env:RALPH_GH_ACCOUNT"
}
Write-Host "prompt: $(Get-Content -Raw $Prompt)"
[Console]::Error.WriteLine('warned')
Write-Host "`e[32mgreen`e[0m ünïcödé 資料界"
if ($Daemon) {
    # Detaches like a daemon: double fork, a new session, no inherited pipes.
    perl -e 'use POSIX qw(setsid); exit if fork; setsid(); exit if fork; open(STDIN, "<", "/dev/null"); open(STDOUT, ">", "/dev/null"); open(STDERR, ">", "/dev/null"); my $n = 0; while (1) { open(my $f, ">", $ARGV[0]) or die; print $f $n++; close $f; select(undef, undef, undef, 0.1); }' $Daemon
    Write-Host "daemon started"
    while ($true) { Start-Sleep -Milliseconds 200 }
}
if ($Beat) {
    $child = Start-Process -FilePath (Get-Process -Id $PID).Path -PassThru -NoNewWindow -ArgumentList @(
        '-NoProfile', '-Command', "while (`$true) { Set-Content -LiteralPath '$Beat' ([DateTime]::UtcNow.Ticks); Start-Sleep -Milliseconds 100 }")
    Write-Host "child $($child.Id)"
    while ($true) { Start-Sleep -Milliseconds 200 }
}
Write-Host "== Iteration 2/$Max"
if ($Stale) {
    # A failed native command the script handled, then a normal end.
    git rev-parse --verify --quiet no-such-ref
    Write-Host "handled"
    return
}
exit $Code
"#;

struct Fixture {
    _root: tempfile::TempDir,
    repository: PathBuf,
    store: Store,
    cycles: u32,
    account: Option<String>,
    /// A stand-in `gh` that prints `stub-token-<user>` for `auth token`, and fails for `missing`.
    gh: PathBuf,
}

const GH_STUB_CMD: &str =
    "@echo off\r\nif \"%6\"==\"missing\" exit /b 1\r\nif \"%1\"==\"auth\" echo stub-token-%6\r\n";
const GH_STUB_SH: &str =
    "#!/bin/sh\n[ \"$6\" = missing ] && exit 1\n[ \"$1\" = auth ] && echo \"stub-token-$6\"\n";

fn gh_stub(folder: &Path) -> PathBuf {
    if cfg!(windows) {
        let stub = folder.join("gh.cmd");
        std::fs::write(&stub, GH_STUB_CMD).unwrap();
        stub
    } else {
        let stub = folder.join("gh");
        std::fs::write(&stub, GH_STUB_SH).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        stub
    }
}

fn fixture() -> Fixture {
    let root = tempfile::Builder::new()
        .prefix("ralph host 界 ")
        .tempdir()
        .unwrap();
    let repository = root.path().join("repo with space");
    std::fs::create_dir_all(&repository).unwrap();
    std::fs::write(repository.join("stub.ps1"), STUB).unwrap();
    let store = Store::at(root.path().join("native-ralph")).unwrap();
    Fixture {
        repository,
        store,
        cycles: 1,
        account: None,
        gh: gh_stub(root.path()),
        _root: root,
    }
}

/// A failed assertion must not leave a detached loop running.
impl Drop for Fixture {
    fn drop(&mut self) {
        let active = self
            .store
            .runs()
            .into_iter()
            .filter(|run| run.state.is_active())
            .map(|run| run.request.id)
            .collect::<Vec<_>>();
        for id in active {
            let _ = self.store.request_stop(&id);
            let deadline = Instant::now() + Duration::from_secs(15);
            while self.store.run(&id).is_some_and(|run| run.state.is_active())
                && Instant::now() < deadline
            {
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

impl Fixture {
    fn start(&mut self, arguments: &[&str]) -> String {
        let id = self.create(arguments);
        spawn_host(
            Path::new(env!("CARGO_BIN_EXE_buddy")),
            &self.store.folder(&id),
        )
        .unwrap();
        id
    }

    /// Records a run without starting its host.
    fn create(&mut self, arguments: &[&str]) -> String {
        let script = self.repository.join("stub.ps1");
        let directory = self.repository.clone();
        let cycles = self.cycles;
        let mut extra = arguments.iter().map(|a| a.to_string()).collect::<Vec<_>>();
        let account = self.account.clone();
        let gh = self.gh.clone();
        let launch = Launch {
            repository: self.repository.clone(),
            prompt: "Fix the bug".into(),
            account: account.clone(),
            ..Launch::default()
        };
        self.store
            .create(launch, None, None, |folder| {
                let mut arguments = vec![
                    "-Max".into(),
                    "2".into(),
                    "-Prompt".into(),
                    folder.join("prompt.md").to_string_lossy().into_owned(),
                ];
                arguments.append(&mut extra);
                Plan {
                    program: pwsh(),
                    git: tool(if cfg!(windows) { "git.exe" } else { "git" }),
                    gh,
                    account,
                    script,
                    arguments,
                    directory,
                    path: std::env::var_os("PATH").unwrap(),
                    cycles,
                }
            })
            .unwrap()
    }

    fn wait(&mut self, id: &str, done: impl Fn(RunState, &[String]) -> bool) -> RunState {
        let deadline = Instant::now() + Duration::from_secs(90);
        loop {
            let state = self.store.run(id).unwrap().state;
            let (output, _) = self.store.output(id);
            if done(state, &output) {
                return state;
            }
            assert!(
                Instant::now() < deadline,
                "run stayed {state:?}; output: {output:?}"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

#[test]
fn a_finished_loop_records_its_output_progress_and_result() {
    let mut fixture = fixture();
    let id = fixture.start(&[]);
    assert_eq!(
        fixture.wait(&id, |state, _| !state.is_active()),
        RunState::Completed
    );
    let run = fixture.store.run(&id).unwrap();
    let status = run.status.unwrap();
    assert_eq!(status.exit_code, Some(0));
    assert_eq!(status.progress.phase, Phase::Iterating);
    assert_eq!(status.progress.iteration, Some(2));
    let (output, _) = fixture.store.output(&id);
    assert!(
        output.contains(&"prompt: Fix the bug".to_string()),
        "{output:?}"
    );
    assert!(
        output.contains(&"[stderr] warned".to_string()),
        "{output:?}"
    );
    assert!(
        output.contains(&"green ünïcödé 資料界".to_string()),
        "{output:?}"
    );
    // The run's folder still names it; a second host for it does nothing.
    let again = Command::new(env!("CARGO_BIN_EXE_buddy"))
        .arg("--ralph-host")
        .arg(fixture.store.folder(&id))
        .status()
        .unwrap();
    assert_eq!(again.code(), Some(4));
    assert_eq!(fixture.store.run(&id).unwrap().state, RunState::Completed);
}

#[test]
fn a_script_that_ends_normally_completes_after_a_handled_native_failure() {
    let mut fixture = fixture();
    let id = fixture.start(&["-Stale"]);
    assert_eq!(
        fixture.wait(&id, |state, _| !state.is_active()),
        RunState::Completed
    );
    let status = fixture.store.run(&id).unwrap().status.unwrap();
    assert_eq!(status.exit_code, Some(0));
    let (output, _) = fixture.store.output(&id);
    assert!(output.contains(&"handled".to_string()), "{output:?}");
}

#[test]
fn a_host_that_starts_after_the_grace_period_never_starts_the_loop() {
    let mut fixture = fixture();
    let id = fixture.create(&[]);
    let folder = fixture.store.folder(&id);
    // As if the machine slept between the launch and the host's start.
    let path = folder.join("request.json");
    let mut request: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    request["createdAt"] = serde_json::json!(request["createdAt"].as_i64().unwrap() - 60_000);
    std::fs::write(&path, serde_json::to_vec(&request).unwrap()).unwrap();
    let mut store = Store::at(fixture.store.root().to_path_buf()).unwrap();
    assert_eq!(store.run(&id).unwrap().state, RunState::Interrupted);
    let late = Command::new(env!("CARGO_BIN_EXE_buddy"))
        .arg("--ralph-host")
        .arg(&folder)
        .status()
        .unwrap();
    assert_eq!(late.code(), Some(5));
    assert!(!folder.join("started").exists());
    assert!(!folder.join("status.json").exists());
    assert_eq!(store.run(&id).unwrap().state, RunState::Interrupted);
}

#[test]
fn a_failing_loop_records_its_exit_code() {
    let mut fixture = fixture();
    let id = fixture.start(&["-Code", "3"]);
    assert_eq!(
        fixture.wait(&id, |state, _| !state.is_active()),
        RunState::Failed
    );
    let status = fixture.store.run(&id).unwrap().status.unwrap();
    assert_eq!(status.exit_code, Some(3));
    assert_eq!(
        status.error.as_deref(),
        Some("The loop exited with code 3.")
    );
}

#[test]
fn a_named_account_signs_the_loop_in_without_saving_its_token() {
    let mut fixture = fixture();
    fixture.account = Some("octo".into());
    let id = fixture.start(&["-Account", "octo"]);
    assert_eq!(
        fixture.wait(&id, |state, _| !state.is_active()),
        RunState::Completed
    );
    let (output, _) = fixture.store.output(&id);
    assert!(
        output.contains(&"[Buddy] GitHub operations run as octo.".to_string()),
        "{output:?}"
    );
    assert!(
        output.contains(&"token True True".to_string()),
        "{output:?}"
    );
    // The scripts keep this account instead of switching to the repository owner's.
    assert!(output.contains(&"account octo".to_string()), "{output:?}");
    // The token lives only in the host and the loop's environment.
    for entry in std::fs::read_dir(fixture.store.folder(&id)).unwrap() {
        let file = entry.unwrap().path();
        if let Ok(bytes) = std::fs::read(&file) {
            assert!(
                !String::from_utf8_lossy(&bytes).contains("stub-token-octo"),
                "{} holds the token",
                file.display()
            );
        }
    }
}

#[test]
fn an_account_without_a_gh_login_fails_before_the_loop_starts() {
    let mut fixture = fixture();
    fixture.account = Some("missing".into());
    let id = fixture.start(&[]);
    assert_eq!(
        fixture.wait(&id, |state, _| !state.is_active()),
        RunState::Failed
    );
    let status = fixture.store.run(&id).unwrap().status.unwrap();
    assert_eq!(
        status.error.as_deref(),
        Some(
            "gh has no login for missing, so the loop did not start. Add it with gh auth login, then retry."
        )
    );
    let (output, _) = fixture.store.output(&id);
    assert!(
        !output.iter().any(|line| line.starts_with("== Iteration")),
        "{output:?}"
    );
}

fn beat(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// Whether the heartbeat file changes. A process that has not written yet is given time
/// to start (PowerShell takes a second or more).
fn beating(path: &Path) -> bool {
    let deadline = Instant::now() + Duration::from_secs(30);
    while beat(path).is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    let before = beat(path);
    std::thread::sleep(Duration::from_millis(1500));
    before.is_some() && beat(path) != before
}

struct Bystander(Child);

impl Drop for Bystander {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn unrelated(beat: &Path) -> Bystander {
    Bystander(Command::new(pwsh())
        .args(["-NoProfile", "-Command"])
        .arg(format!(
            "while ($true) {{ Set-Content -LiteralPath '{}' ([DateTime]::UtcNow.Ticks); Start-Sleep -Milliseconds 100 }}",
            beat.display()
        ))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .spawn()
        .unwrap())
}

/// A process that daemonized (double fork and `setsid`) has left the run's session, so
/// signalling the session misses it; the run's marker in its environment does not.
#[cfg(unix)]
#[test]
fn stop_ends_a_process_that_left_the_loops_session() {
    let mut fixture = fixture();
    let daemon = fixture.repository.join("daemon beat");
    let other = fixture.repository.join("other beat");
    let _bystander = unrelated(&other);
    let id = fixture.start(&["-Daemon", daemon.to_str().unwrap()]);
    fixture.wait(&id, |state, output| {
        state == RunState::Running && output.iter().any(|line| line == "daemon started")
    });
    assert!(beating(&daemon), "the daemon should be running");
    fixture.store.request_stop(&id).unwrap();
    assert_eq!(
        fixture.wait(&id, |state, _| !state.is_active()),
        RunState::Stopped
    );
    assert!(
        !beating(&daemon),
        "the daemon outside the session survived Stop"
    );
    assert!(beating(&other), "an unrelated process was stopped");
}

#[test]
fn stop_ends_the_loops_own_process_tree_and_nothing_else() {
    let mut fixture = fixture();
    let owned = fixture.repository.join("owned beat");
    let other = fixture.repository.join("other beat");
    let _bystander = unrelated(&other);
    let id = fixture.start(&["-Beat", owned.to_str().unwrap()]);
    fixture.wait(&id, |state, output| {
        state == RunState::Running && output.iter().any(|line| line.starts_with("child "))
    });
    assert!(
        beating(&owned),
        "the loop's child process should be running"
    );
    fixture.store.request_stop(&id).unwrap();
    assert_eq!(
        fixture.wait(&id, |state, _| !state.is_active()),
        RunState::Stopped
    );
    assert!(!beating(&owned), "the loop's child process survived Stop");
    assert!(beating(&other), "an unrelated process was stopped");
    let (output, _) = fixture.store.output(&id);
    assert_eq!(output.last().map(String::as_str), Some("[Buddy] Stopped."));
}

#[test]
fn a_dead_host_leaves_an_interrupted_run_and_takes_its_loop_along() {
    let mut fixture = fixture();
    let owned = fixture.repository.join("owned beat");
    let id = fixture.start(&["-Beat", owned.to_str().unwrap()]);
    fixture.wait(&id, |state, output| {
        state == RunState::Running && output.iter().any(|line| line.starts_with("child "))
    });
    assert!(beating(&owned));
    let host = fixture.store.run(&id).unwrap().status.unwrap().host_pid;
    let killed = if cfg!(windows) {
        Command::new("taskkill")
            .args(["/F", "/PID", &host.to_string()])
            .stdout(Stdio::null())
            .status()
    } else {
        Command::new("kill")
            .args(["-9", &host.to_string()])
            .status()
    }
    .unwrap();
    assert!(killed.success());
    assert_eq!(
        fixture.wait(&id, |state, _| state == RunState::Interrupted),
        RunState::Interrupted
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while beating(&owned) {
        assert!(Instant::now() < deadline, "the loop outlived its host");
    }
    // An interrupted run is shown, never resumed: a new host refuses it.
    let again = Command::new(env!("CARGO_BIN_EXE_buddy"))
        .arg("--ralph-host")
        .arg(fixture.store.folder(&id))
        .status()
        .unwrap();
    assert_eq!(again.code(), Some(4));
}

fn git(directory: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .args([
            "-c",
            "user.name=Buddy Test",
            "-c",
            "user.email=test@example.invalid",
        ])
        .args(arguments)
        .current_dir(directory)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {arguments:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

#[test]
fn repeat_cycles_fast_forward_the_checkout_between_cycles() {
    let mut fixture = fixture();
    let parent = fixture.repository.parent().unwrap().to_path_buf();
    let remote = parent.join("remote.git");
    git(
        &parent,
        &[
            "init",
            "-q",
            "--bare",
            "-b",
            "main",
            remote.to_str().unwrap(),
        ],
    );
    let repository = fixture.repository.clone();
    git(&repository, &["init", "-q", "-b", "main"]);
    git(&repository, &["add", "stub.ps1"]);
    git(&repository, &["commit", "-q", "-m", "stub"]);
    git(
        &repository,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(&repository, &["push", "-q", "-u", "origin", "main"]);
    // Work merged upstream while the first cycle ran.
    let other = parent.join("other clone");
    git(
        &parent,
        &[
            "clone",
            "-q",
            remote.to_str().unwrap(),
            other.to_str().unwrap(),
        ],
    );
    std::fs::write(other.join("merged.txt"), "merged").unwrap();
    git(&other, &["add", "merged.txt"]);
    git(&other, &["commit", "-q", "-m", "merged"]);
    git(&other, &["push", "-q", "origin", "HEAD:main"]);
    fixture.cycles = 2;
    let id = fixture.start(&[]);
    assert_eq!(
        fixture.wait(&id, |state, _| !state.is_active()),
        RunState::Completed
    );
    assert_eq!(
        git(&repository, &["rev-parse", "HEAD"]),
        git(&other, &["rev-parse", "HEAD"])
    );
    let (output, _) = fixture.store.output(&id);
    assert!(
        output.contains(
            &"[Buddy] Fast-forwarding from origin/main before the next cycle.".to_string()
        ),
        "{output:?}"
    );
    assert!(
        output.contains(&"[Buddy] Cycle 2 of 2 started.".to_string()),
        "{output:?}"
    );
}

#[test]
fn an_upstream_lost_between_cycles_fails_the_run() {
    let mut fixture = fixture();
    let parent = fixture.repository.parent().unwrap().to_path_buf();
    let remote = parent.join("remote.git");
    git(
        &parent,
        &[
            "init",
            "-q",
            "--bare",
            "-b",
            "main",
            remote.to_str().unwrap(),
        ],
    );
    let repository = fixture.repository.clone();
    git(&repository, &["init", "-q", "-b", "main"]);
    git(&repository, &["add", "stub.ps1"]);
    git(&repository, &["commit", "-q", "-m", "stub"]);
    git(
        &repository,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(&repository, &["push", "-q", "-u", "origin", "main"]);
    fixture.cycles = 2;
    // The first cycle removes the branch's upstream.
    let id = fixture.start(&["-Unset"]);
    assert_eq!(
        fixture.wait(&id, |state, _| !state.is_active()),
        RunState::Failed
    );
    let status = fixture.store.run(&id).unwrap().status.unwrap();
    assert_eq!(
        status.error.as_deref(),
        Some(
            "The checked-out branch no longer tracks an upstream, so the next cycle cannot start from the merged work. Set its upstream, then retry."
        )
    );
    let (output, _) = fixture.store.output(&id);
    assert!(
        !output.contains(&"[Buddy] Cycle 2 of 2 started.".to_string()),
        "{output:?}"
    );
}
