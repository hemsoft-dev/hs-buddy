//! When Buddy belongs to a Windows Job that ends its processes on close and forbids leaving
//! it, a loop's host starts inside that Job. Closing Buddy leaves it running, but closing
//! the program that owns the Job ends it, and the run's log says so. This test joins such a
//! Job, so it runs in its own test process.
#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use buddy_core::ralph::host::{JOB_WARNING, spawn_host};
use buddy_core::ralph::launch::{Launch, Plan};
use buddy_core::ralph::store::{RunState, Store};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject,
};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

fn tool(name: &str) -> PathBuf {
    std::env::var_os("PATH")
        .and_then(|path| {
            std::env::split_paths(&path)
                .map(|directory| directory.join(name))
                .find(|path| path.is_file())
        })
        .unwrap_or_else(|| panic!("{name} is required on PATH for the Ralph host tests"))
}

#[test]
fn a_host_inside_a_kill_on_close_job_says_what_ends_it() {
    // SAFETY: plain Job object calls on a fresh Job and this process's pseudo-handle. The
    // Job handle stays open until this test process ends, as a launcher's would.
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        assert!(!job.is_null());
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        assert!(
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                std::mem::size_of_val(&info) as u32,
            ) != 0
        );
        assert!(AssignProcessToJobObject(job, GetCurrentProcess()) != 0);
    }
    let root = tempfile::tempdir().unwrap();
    let repository = root.path().join("repo");
    std::fs::create_dir_all(&repository).unwrap();
    let script = repository.join("stub.ps1");
    std::fs::write(&script, "Write-Host 'stub done'\n").unwrap();
    let mut store = Store::at(root.path().join("native-ralph")).unwrap();
    let launch = Launch {
        repository: repository.clone(),
        ..Launch::default()
    };
    let id = store
        .create(launch, None, None, |_| Plan {
            program: tool("pwsh.exe"),
            git: tool("git.exe"),
            gh: PathBuf::new(),
            account: None,
            base: None,
            script: script.clone(),
            arguments: vec![],
            directory: repository.clone(),
            path: std::env::var_os("PATH").unwrap(),
            cycles: 1,
        })
        .unwrap();
    spawn_host(Path::new(env!("CARGO_BIN_EXE_buddy")), &store.folder(&id)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(90);
    while store.run(&id).unwrap().state.is_active() {
        assert!(Instant::now() < deadline, "the run did not finish");
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(store.run(&id).unwrap().state, RunState::Completed);
    let (output, _) = store.output(&id);
    assert!(output.contains(&JOB_WARNING.to_string()), "{output:?}");
    assert!(output.contains(&"stub done".to_string()), "{output:?}");
}
