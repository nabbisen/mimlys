//! Windows process creation: exit codes, the standard input of a host tool,
//! and the handles of every error path (handoff 011n items 4, 5 and 6;
//! REQ-CONF-006, REQ-CONF-005).

use super::*;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use windows_sys::Win32::System::Threading::GetProcessHandleCount;

fn cmd_exe() -> PathBuf {
    PathBuf::from(std::env::var_os("SystemRoot").expect("SystemRoot"))
        .join("System32")
        .join("cmd.exe")
}

/// `cmd.exe /c <line>`, with the standard input as given and the job if any.
fn run(line: &str, stdin: Stdin, job: Option<&Job>) -> Result<RawChild, String> {
    let root = std::env::var("SystemRoot").expect("SystemRoot");
    spawn(SpawnOptions {
        program: &cmd_exe(),
        args: &["/c".to_owned(), line.to_owned()],
        env: &[("SystemRoot".to_owned(), root)],
        current_dir: None,
        container: None,
        job,
        restricted_token: false,
        stdin,
    })
}

/// Polls the child until it has ended or `limit` has passed.
fn ended_within(child: &mut RawChild, limit: Duration) -> Option<ExitStatus> {
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().expect("try_wait") {
            return Some(status);
        }
        if started.elapsed() > limit {
            return None;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// REQ-CONF-006 (handoff 011n item 4): a process that exits with 259, the
/// value `STILL_ACTIVE` stands for while it runs, has ended and its exit code
/// is 259. It used to be read as still running for ever.
#[test]
fn req_conf_006_exit_code_259_is_a_final_exit_code() {
    for code in [0u32, 3, 259, 260] {
        let mut child = run(&format!("exit {code}"), Stdin::Closed, None).expect("spawns");
        let status = ended_within(&mut child, Duration::from_secs(60))
            .unwrap_or_else(|| panic!("a process that exited with {code} is still running"));
        assert_eq!(status.code(), Some(code as i32), "exit code {code}");
    }
}

/// REQ-CONF-006 (handoff 011n item 5): a host tool's standard input is closed
/// at the spawn, so a program that reads it finds the end of the input at
/// once and ends (the parent holds no end of it); the worker's own pipe
/// (`Stdin::Piped`) stays open for its frames.
#[test]
fn req_conf_006_a_closed_stdin_reads_the_end_of_input_at_once() {
    // `findstr` reads its standard input to the end, then ends (exit code 1: no match).
    let mut closed = run("findstr x", Stdin::Closed, None).expect("spawns");
    assert!(closed.stdin.is_none(), "the parent holds no end of it");
    let status = ended_within(&mut closed, Duration::from_secs(60))
        .expect("a program reading a closed standard input ends");
    assert_eq!(status.code(), Some(1));
    // Contrast: the worker's pipe is open and the parent holds its writing end
    // (that a program reading it waits is shown on the host-tool path, by the
    // probe tool, in `m6b_host_tools`).
    let mut open = run("exit 0", Stdin::Piped, None).expect("spawns");
    assert!(open.stdin.is_some());
    assert!(ended_within(&mut open, Duration::from_secs(60)).is_some());
}

fn handle_count() -> u32 {
    let mut count = 0u32;
    // SAFETY: the pseudo-handle of the current process is always valid; `count` is a valid out pointer.
    let ok = unsafe { GetProcessHandleCount(GetCurrentProcess(), &mut count) };
    assert_ne!(ok, 0);
    count
}

/// REQ-CONF-006 (handoff 011n item 6): a failure at any step of the spawn
/// (each pipe, the handle flag, the attribute list and its entries, the
/// creation, the job, the resume) leaves no handle open. The count of the
/// process's handles is read before and after many failures at each step;
/// a handle left behind by each failure shows as that many more. The test
/// runs in a process of its own (this binary again, with this one test), so
/// that nothing else opens or closes a handle while it counts.
#[test]
fn req_conf_006_a_failed_spawn_leaves_no_handle_open() {
    const CHILD: &str = "MIMLYS_SPAWN_LEAK_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .env(CHILD, "1")
            .args([
                "--exact",
                "confine::windows::tests::req_conf_006_a_failed_spawn_leaves_no_handle_open",
                "--test-threads=1",
                "--nocapture",
            ])
            .status()
            .unwrap();
        assert!(status.success(), "the counting run failed");
        return;
    }
    let job = Job::new().unwrap();
    // Every step the spawn names, in order; the container's step needs a container SID.
    let steps = [
        "stdin_pipe",
        "stdout_pipe",
        "stderr_pipe",
        "handle_flag",
        "attribute_list",
        "attribute_handles",
        "create_process",
        "job_assign",
        "resume_thread",
    ];
    // A first, successful spawn: the system's lazy handles (the first process, the first pipe) exist before counting.
    let mut warm = run("exit 0", Stdin::Closed, Some(&job)).unwrap();
    assert!(ended_within(&mut warm, Duration::from_secs(60)).is_some());
    drop(warm);
    for step in steps {
        *FAIL_AT.lock().unwrap() = Some(step);
        // One failure first: the step is reached and fails with its own text.
        let error = run("exit 0", Stdin::Piped, Some(&job))
            .map(|_| ())
            .unwrap_err();
        assert!(error.contains(step), "{step}: {error}");
        let before = handle_count();
        for _ in 0..40 {
            assert!(run("exit 0", Stdin::Piped, Some(&job)).is_err(), "{step}");
        }
        let after = handle_count();
        assert!(
            after < before + 20,
            "{step}: {} handles were left open by 40 failures ({before} before, {after} after)",
            after.saturating_sub(before)
        );
    }
    *FAIL_AT.lock().unwrap() = None;
    // And the ordinary path closes what it opens too.
    let before = handle_count();
    for _ in 0..20 {
        let mut child = run("exit 0", Stdin::Closed, Some(&job)).unwrap();
        assert!(ended_within(&mut child, Duration::from_secs(60)).is_some());
    }
    let after = handle_count();
    assert!(after < before + 20, "{before} before, {after} after");
}

/// REQ-CONF-008 (handoff 011o, D1), on the Windows job: every spelling of one
/// data directory (another case, a trailing separator, the verbatim prefix, a
/// `..` detour, a junction) is one container; another directory is another.
/// The name is not the one 0.10 gave (the spelling's hash), which is how the
/// migration finds the old container.
#[test]
fn req_conf_008_two_spellings_of_one_data_directory_give_one_profile() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    std::fs::create_dir_all(data.join("sub")).unwrap();
    let junction = root.path().join("link");
    let made = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&junction)
        .arg(&data)
        .output()
        .unwrap();
    assert!(
        made.status.success(),
        "mklink: {}",
        String::from_utf8_lossy(&made.stdout)
    );
    let text = data.to_string_lossy().into_owned();
    let spellings: Vec<PathBuf> = vec![
        data.clone(),
        PathBuf::from(format!("{text}\\")),
        PathBuf::from(text.to_uppercase()),
        PathBuf::from(format!(r"\\?\{text}")),
        data.join("sub").join(".."),
        junction.clone(),
    ];
    let first = profile_name(&data);
    for spelling in &spellings {
        assert_eq!(
            profile_name(spelling),
            first,
            "{} names another container",
            spelling.display()
        );
    }
    assert_ne!(profile_name(&data.join("sub")), first);
    assert_ne!(legacy_profile_name(&data), first);
    // 0.10's name differed between two of these spellings (the separator).
    assert_ne!(
        legacy_profile_name(&data),
        legacy_profile_name(&PathBuf::from(format!("{text}\\")))
    );
    // A directory that does not exist has no identity: the old name stands in.
    let missing = root.path().join("missing");
    assert_eq!(profile_name(&missing), legacy_profile_name(&missing));
    std::fs::remove_dir(&junction).unwrap();
}
