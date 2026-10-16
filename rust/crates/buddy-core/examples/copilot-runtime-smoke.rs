//! Installed-app runtime extraction without opening a window or sending a turn.
use std::path::PathBuf;

fn main() {
    let mut args = std::env::args_os().skip(1);
    let executable = PathBuf::from(args.next().expect("installed app path"));
    let root = PathBuf::from(args.next().expect("owned runtime directory"));
    assert!(args.next().is_none(), "unexpected argument");
    let runtime = tokio::runtime::Runtime::new().expect("I/O runtime");
    match runtime.block_on(buddy_core::copilot_runtime::prepare(executable, root)) {
        Ok(program) => {
            assert!(program.program().is_file());
            println!(
                "RUNTIME PASS ownedHelper=true verifiedBundle=true unauthenticated=true noSessions=true"
            );
        }
        Err(_) => {
            eprintln!("RUNTIME FAIL assistant preparation did not qualify");
            std::process::exit(1);
        }
    }
}
