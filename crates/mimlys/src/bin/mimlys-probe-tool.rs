//! The probe tool of the confinement tests (REQ-TEST-013): a host tool that
//! tries what its first argument says and reports in its exit code and
//! output. Built only with the `test-bins` feature; never part of a release.
//!
//! `read <path>`, `write <path>`, `connect <addr>`, `env <name>`,
//! `sleep <secs>`, `setsid-sleep <secs>`, `detach <secs>` (Unix but macOS: a sleeper in a session of its own, then the tool ends), `fork <n> [hold_ms]`, `alloc <mib>`,
//! `alloc-touch <mib>` (resident memory: every page written), `connect-name
//! <host:port>` (resolve and connect), `cwd`, `ok`.
//!
//! Linux only (REQ-CONF-012, REQ-CONF-011; the syscalls are in
//! `mimlys_exec::confine::linux::probe`, the one place the tests may reach
//! foreign-function code): `uring-socket` (an `AF_INET` socket created
//! through `IORING_OP_SOCKET`), `uring-connect <addr>` (a socket and a
//! connection through `io_uring`), `vm-read <pid|child> [hex-address]`
//! (`process_vm_readv`; `child` starts a copy of this tool that holds a known
//! string and reads it), `hold` (prints the address of that string and
//! sleeps), `read-proc <ENVVAR> <file>` (reads `/proc/<pid in ENVVAR>/<file>`).
//! Each prints one line and exits non-zero when the thing could not be done.
//!
//! Also, for the macOS clause suite (RFC 011 Amendment 2; REQ-CONF-002,
//! REQ-CONF-011, REQ-CONF-012): `getaddrinfo <name>` (a name lookup, which
//! on macOS goes through a mach service and a Unix socket, an egress
//! channel), `unix-connect <path>` (connect to an existing Unix socket), `unix-socket <path>` (bind, listen and connect a Unix socket
//! there), `id` (`/usr/bin/id -u`: an ordinary `exec`), `sudo-id`
//! (`/usr/bin/sudo -n /usr/bin/id -u`: an `exec` of a setuid binary), and on
//! macOS only, from `mimlys_exec::confine::macos::probe`: `procargs
//! <PIDVAR> <needle>` (`sysctl KERN_PROCARGS2` of the pid in `PIDVAR`, and
//! whether `needle` is in its arguments and environment), `procinfo
//! <PIDVAR>` (`proc_pidinfo`) and `signal0 <PIDVAR>` (signal 0).

use std::io::Write;

/// `vm-read <pid|child> [hex-address]`: `process_vm_readv` of another
/// process. `child` starts `hold` (a descendant, so the kernel's own checks
/// allow the read when no filter denies it) and reads the string it holds.
#[cfg(target_os = "linux")]
fn vm_read(args: &[String]) -> i32 {
    use mimlys_exec::confine::linux::probe::vm_read;
    use std::io::BufRead;
    let target = args.get(1).map(String::as_str).unwrap_or("");
    let (pid, addr, mut child) = if target == "child" {
        let mut child = std::process::Command::new(std::env::current_exe().expect("own path"))
            .arg("hold")
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("spawn hold");
        let mut line = String::new();
        std::io::BufReader::new(child.stdout.take().expect("piped"))
            .read_line(&mut line)
            .expect("hold prints its address");
        let addr = usize::from_str_radix(line.trim().trim_start_matches("addr "), 16)
            .expect("hex address");
        (child.id() as i32, addr, Some(child))
    } else {
        let pid: i32 = target.parse().unwrap_or(0);
        let addr = args
            .get(2)
            .and_then(|a| usize::from_str_radix(a.trim_start_matches("0x"), 16).ok())
            .unwrap_or(0x1000);
        (pid, addr, None)
    };
    let result = vm_read(pid, addr, 20);
    if let Some(child) = child.as_mut() {
        let _ = child.kill();
        let _ = child.wait();
    }
    match result {
        Ok(bytes) => {
            println!("vm-read read {:?}", String::from_utf8_lossy(&bytes));
            0
        }
        Err(errno) => {
            println!(
                "vm-read failed: errno={errno} ({})",
                std::io::Error::from_raw_os_error(errno)
            );
            3
        }
    }
}

/// The pid held by the environment variable `args[1]`, else the exit code to give.
#[cfg(target_os = "macos")]
fn pid_from_env(args: &[String]) -> Result<i32, i32> {
    let var = args.get(1).map(String::as_str).unwrap_or("");
    match std::env::var(var).ok().and_then(|v| v.parse::<i32>().ok()) {
        Some(pid) => Ok(pid),
        None => {
            println!("{var} unset or not a pid");
            Err(2)
        }
    }
}

/// Runs a program and prints its first line of output, or what refused it.
#[cfg(unix)]
fn run_and_print(name: &str, program: &str, args: &[&str]) -> i32 {
    match std::process::Command::new(program).args(args).output() {
        Ok(out) => {
            let line = String::from_utf8_lossy(&out.stdout);
            println!("{name} ran: {}", line.trim());
            if out.status.success() { 0 } else { 4 }
        }
        Err(e) => {
            println!("{name} failed: {}", e.kind());
            3
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("ok") => {
            println!("ok");
            0
        }
        Some("cwd") => {
            println!(
                "{}",
                std::env::current_dir()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default()
            );
            0
        }
        Some("read") => match std::fs::read(args.get(1).map(String::as_str).unwrap_or("")) {
            Ok(bytes) => {
                println!("read {} bytes", bytes.len());
                0
            }
            Err(e) => {
                println!("read failed: {}", e.kind());
                3
            }
        },
        Some("write") => {
            match std::fs::write(args.get(1).map(String::as_str).unwrap_or(""), b"x") {
                Ok(()) => {
                    println!("wrote");
                    0
                }
                Err(e) => {
                    println!("write failed: {}", e.kind());
                    3
                }
            }
        }
        Some("connect") => {
            let addr = args.get(1).map(String::as_str).unwrap_or("127.0.0.1:9");
            match std::net::TcpStream::connect_timeout(
                &addr.parse().expect("addr"),
                std::time::Duration::from_millis(500),
            ) {
                Ok(_) => {
                    println!("connected");
                    0
                }
                Err(e) => {
                    println!("connect failed: {}", e.kind());
                    3
                }
            }
        }
        Some("getaddrinfo") => {
            use std::net::ToSocketAddrs;
            let name = args.get(1).map(String::as_str).unwrap_or("localhost");
            match (name, 443).to_socket_addrs() {
                Ok(addrs) => {
                    println!("getaddrinfo resolved {} addresses", addrs.count());
                    0
                }
                Err(e) => {
                    println!("getaddrinfo failed: {}", e.kind());
                    3
                }
            }
        }
        #[cfg(unix)]
        Some("unix-socket") => {
            let path = args.get(1).map(String::as_str).unwrap_or("");
            let _ = std::fs::remove_file(path);
            match std::os::unix::net::UnixListener::bind(path) {
                Err(e) => {
                    println!("unix-socket bind failed: {}", e.kind());
                    3
                }
                Ok(_listener) => match std::os::unix::net::UnixStream::connect(path) {
                    Ok(_) => {
                        println!("unix-socket bound and connected");
                        let _ = std::fs::remove_file(path);
                        0
                    }
                    Err(e) => {
                        println!("unix-socket connect failed: {}", e.kind());
                        3
                    }
                },
            }
        }
        #[cfg(unix)]
        Some("unix-connect") => {
            let path = args.get(1).map(String::as_str).unwrap_or("");
            match std::os::unix::net::UnixStream::connect(path) {
                Ok(_) => {
                    println!("unix-connect connected");
                    0
                }
                Err(e) => {
                    println!("unix-connect failed: {}", e.kind());
                    3
                }
            }
        }
        #[cfg(unix)]
        Some("id") => run_and_print("id", "/usr/bin/id", &["-u"]),
        #[cfg(unix)]
        Some("sudo-id") => run_and_print("sudo-id", "/usr/bin/sudo", &["-n", "/usr/bin/id", "-u"]),
        // `xcrun --find <tool>`, as a build's `cc` shim or linker driver asks
        // it; the exit code is 0 only if `xcrun` resolved the tool.
        #[cfg(target_os = "macos")]
        Some("xcrun-find") => {
            let tool = args.get(1).map(String::as_str).unwrap_or("");
            run_and_print("xcrun-find", "/usr/bin/xcrun", &["--find", tool])
        }
        #[cfg(target_os = "macos")]
        Some("procargs") => {
            use mimlys_exec::confine::macos::probe::procargs_contains;
            match pid_from_env(&args) {
                Err(code) => code,
                Ok(pid) => {
                    let needle = args.get(2).map(String::as_str).unwrap_or("");
                    match procargs_contains(pid, needle) {
                        Ok(seen) => {
                            println!(
                                "procargs read; the needle is visible: {}",
                                if seen { "yes" } else { "no" }
                            );
                            0
                        }
                        Err(errno) => {
                            println!(
                                "procargs failed: errno={errno} ({})",
                                std::io::Error::from_raw_os_error(errno)
                            );
                            3
                        }
                    }
                }
            }
        }
        #[cfg(target_os = "macos")]
        Some("procinfo") => match pid_from_env(&args) {
            Err(code) => code,
            Ok(pid) => match mimlys_exec::confine::macos::probe::process_info(pid) {
                Ok(()) => {
                    println!("procinfo answered");
                    0
                }
                Err(errno) => {
                    println!(
                        "procinfo failed: errno={errno} ({})",
                        std::io::Error::from_raw_os_error(errno)
                    );
                    3
                }
            },
        },
        #[cfg(target_os = "macos")]
        Some("signal0") => match pid_from_env(&args) {
            Err(code) => code,
            Ok(pid) => match mimlys_exec::confine::macos::probe::signal_zero(pid) {
                Ok(()) => {
                    println!("signal0 allowed");
                    0
                }
                Err(errno) => {
                    println!(
                        "signal0 failed: errno={errno} ({})",
                        std::io::Error::from_raw_os_error(errno)
                    );
                    3
                }
            },
        },
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        Some("sweeper-demo") => {
            // The sweeper's pipe, alone (REQ-CONF-009): `sweeper-demo <marker>
            // <hold_secs> <slow_ms> <child_secs>`. Forks the sweeper (it writes
            // `<marker>` at end of file, after `slow_ms`), prints its pid, then,
            // when `<child_secs>` is not 0, starts a `sleep <child_secs>` child
            // and prints `child <pid>`; holds for `hold_secs` and exits.
            // Single-threaded, as the sweeper requires. The tests decide by the
            // state of these processes, not by elapsed time (handoff 011i).
            let marker = args.get(1).cloned().unwrap_or_default();
            let hold: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
            let slow: u64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(0);
            let child_secs: u64 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(0);
            let sweeper = mimlys_exec::confine::sweeper::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(slow));
                let _ = std::fs::write(&marker, "end of file");
            });
            match sweeper {
                Ok(sweeper) => {
                    println!("sweeper {}", sweeper.pid);
                    let _ = std::io::stdout().flush();
                    if child_secs > 0 {
                        let child = std::process::Command::new("sleep")
                            .arg(child_secs.to_string())
                            .stdin(std::process::Stdio::null())
                            .stdout(std::process::Stdio::null())
                            .stderr(std::process::Stdio::null())
                            .spawn();
                        if let Ok(child) = child {
                            println!("child {}", child.id());
                            let _ = std::io::stdout().flush();
                        }
                    }
                    std::thread::sleep(std::time::Duration::from_secs(hold));
                    0
                }
                Err(e) => {
                    println!("sweeper failed: {e}");
                    3
                }
            }
        }
        Some("env") => {
            let name = args.get(1).map(String::as_str).unwrap_or("");
            match std::env::var(name) {
                Ok(v) => {
                    println!("{name}={v}");
                    0
                }
                Err(_) => {
                    println!("{name} unset");
                    3
                }
            }
        }
        Some("sleep") => {
            let secs: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(5);
            std::thread::sleep(std::time::Duration::from_secs(secs));
            0
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        Some("detach") => {
            // `detach <secs>`: starts `setsid sleep <secs>` in the background and
            // ends at once, so that what is left is a process in a session of its
            // own, outside the tool's group, with no parent to wait for it.
            let secs = args.get(1).map(String::as_str).unwrap_or("30");
            match std::process::Command::new("sh")
                .args(["-c", &format!("setsid sleep {secs} >/dev/null 2>&1 &")])
                .status()
            {
                Ok(_) => {
                    println!("detached");
                    0
                }
                Err(e) => {
                    println!("detach failed: {}", e.kind());
                    3
                }
            }
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        Some("setsid-sleep") => {
            // A child that leaves the process group, then everyone sleeps.
            let secs = args.get(1).map(String::as_str).unwrap_or("30");
            let _ = std::process::Command::new("sh")
                .args(["-c", &format!("setsid sleep {secs} & sleep {secs}")])
                .spawn()
                .and_then(|mut c| c.wait());
            0
        }
        #[cfg(target_os = "macos")]
        Some("setsid-sleep") => {
            // macOS has no `setsid` command: a shell in a session of its own
            // starts a sleeper in the background and exits at once, so the
            // sleeper is an orphan re-parented to launchd, outside the tool's
            // process group and session (it is still a member of the sandbox).
            // Then the tool sleeps too.
            let secs = args.get(1).map(String::as_str).unwrap_or("30");
            let script = format!("/bin/sleep {secs} &");
            match mimlys_exec::confine::macos::probe::spawn_in_new_session(
                "/bin/sh",
                &["-c", &script],
            ) {
                Ok(mut shell) => {
                    let _ = shell.wait();
                }
                Err(e) => println!("spawn failed: {}", e.kind()),
            }
            let _ = std::io::stdout().flush();
            std::thread::sleep(std::time::Duration::from_secs(secs.parse().unwrap_or(30)));
            0
        }
        #[cfg(target_os = "macos")]
        Some("orphan") => {
            // As `setsid-sleep`, but the tool itself ends at once: what it
            // leaves behind is an orphan of the sandbox with no parent to wait for.
            let secs = args.get(1).map(String::as_str).unwrap_or("30");
            let script = format!("/bin/sleep {secs} >/dev/null 2>&1 &");
            match mimlys_exec::confine::macos::probe::spawn_in_new_session(
                "/bin/sh",
                &["-c", &script],
            ) {
                Ok(mut shell) => {
                    let _ = shell.wait();
                    0
                }
                Err(e) => {
                    println!("spawn failed: {}", e.kind());
                    3
                }
            }
        }
        #[cfg(target_os = "macos")]
        Some("visible") => {
            // The two-workers test: `visible <secs>`. Starts a sleeper in a
            // session of its own, writes `mine.txt` (this tool's pid and the
            // sleeper's) and, every 200 ms for `secs`, `seen.txt` (the pids
            // this tool can inspect), in the working directory.
            let secs: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(5);
            match mimlys_exec::confine::macos::probe::spawn_in_new_session(
                "/bin/sleep",
                &[&secs.to_string()],
            ) {
                Err(e) => {
                    println!("spawn failed: {}", e.kind());
                    3
                }
                Ok(sleeper) => {
                    let _ = std::fs::write(
                        "mine.txt",
                        format!("{} {}", std::process::id(), sleeper.id()),
                    );
                    let until = std::time::Instant::now() + std::time::Duration::from_secs(secs);
                    while std::time::Instant::now() < until {
                        let seen: Vec<String> = mimlys_exec::confine::macos::probe::visible_pids()
                            .iter()
                            .map(i32::to_string)
                            .collect();
                        let _ = std::fs::write("seen.tmp", seen.join(" "));
                        let _ = std::fs::rename("seen.tmp", "seen.txt");
                        std::thread::sleep(std::time::Duration::from_millis(200));
                    }
                    0
                }
            }
        }
        #[cfg(target_os = "macos")]
        Some("poll-time") => {
            // The cost of one poll of the watchdogs, measured from inside the
            // sandbox (the same calls as the worker's `members()`): `poll-time
            // <n>` lists every pid and reads the ones the profile lets it read,
            // n times, and prints the count of pids and the times in ms.
            let n: usize = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(50);
            let mut times: Vec<f64> = Vec::new();
            let mut pids = 0;
            for _ in 0..n {
                let started = std::time::Instant::now();
                let listed = mimlys_exec::confine::macos::probe::visible_pids();
                times.push(started.elapsed().as_secs_f64() * 1000.0);
                pids = listed.len();
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            let total: f64 = times.iter().sum();
            let max = times.iter().cloned().fold(0.0, f64::max);
            let min = times.iter().cloned().fold(f64::MAX, f64::min);
            println!(
                "polls {n} members_read {pids} min_ms {min:.2} mean_ms {:.2} max_ms {max:.2}",
                total / n as f64
            );
            0
        }
        #[cfg(target_os = "macos")]
        Some("proclist") => match mimlys_exec::confine::macos::probe::process_list_size() {
            Ok(bytes) => {
                println!("proclist read: {bytes} bytes");
                0
            }
            Err(errno) => {
                println!(
                    "proclist failed: errno={errno} ({})",
                    std::io::Error::from_raw_os_error(errno)
                );
                3
            }
        },
        #[cfg(windows)]
        Some("setsid-sleep") => {
            // A detached child in a new process group (the closest thing to
            // `setsid`), its pid printed for the test, then everyone sleeps.
            use std::os::windows::process::CommandExt;
            let secs = args.get(1).map(String::as_str).unwrap_or("30");
            let exe = std::env::current_exe().expect("own path");
            match std::process::Command::new(exe)
                .args(["sleep", secs])
                .creation_flags(0x0000_0008 | 0x0000_0200)
                .spawn()
            {
                Ok(child) => println!("child {}", child.id()),
                Err(e) => println!("spawn failed: {}", e.kind()),
            }
            let _ = std::io::stdout().flush();
            std::thread::sleep(std::time::Duration::from_secs(secs.parse().unwrap_or(30)));
            0
        }
        Some("fork") => {
            let n: usize = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(10);
            let exe = std::env::current_exe().expect("own path");
            let mut children = Vec::new();
            let mut spawned = 0;
            for _ in 0..n {
                match std::process::Command::new(&exe)
                    .args(["sleep", "5"])
                    .spawn()
                {
                    Ok(c) => {
                        children.push(c);
                        spawned += 1;
                    }
                    Err(_) => break,
                }
            }
            println!("spawned {spawned}");
            let _ = std::io::stdout().flush();
            // Optionally hold the children (milliseconds), so that a watchdog
            // polling the tree has time to see them.
            if let Some(ms) = args.get(2).and_then(|s| s.parse::<u64>().ok()) {
                std::thread::sleep(std::time::Duration::from_millis(ms));
            }
            for mut c in children {
                let _ = c.kill();
                let _ = c.wait();
            }
            if spawned == n { 0 } else { 3 }
        }
        #[cfg(target_os = "linux")]
        Some("uring-socket") => match mimlys_exec::confine::linux::probe::uring_socket() {
            Ok(fd) => {
                println!("uring-socket created fd {fd}");
                0
            }
            Err(why) => {
                println!("uring-socket failed: {why}");
                3
            }
        },
        #[cfg(target_os = "linux")]
        Some("uring-connect") => {
            let addr = args.get(1).map(String::as_str).unwrap_or("127.0.0.1:9");
            match mimlys_exec::confine::linux::probe::uring_connect(addr.parse().expect("addr")) {
                Ok(()) => {
                    println!("uring-connect connected");
                    0
                }
                Err(why) => {
                    println!("uring-connect failed: {why}");
                    3
                }
            }
        }
        #[cfg(target_os = "linux")]
        Some("hold") => {
            // A known string at a known address, for `vm-read child`.
            static HELD: &[u8] = b"mimlys-vm-read-probe";
            println!("addr {:x}", HELD.as_ptr() as usize);
            let _ = std::io::stdout().flush();
            std::thread::sleep(std::time::Duration::from_secs(30));
            0
        }
        #[cfg(target_os = "linux")]
        Some("vm-read") => vm_read(&args),
        #[cfg(target_os = "linux")]
        Some("read-proc") => {
            let var = args.get(1).map(String::as_str).unwrap_or("");
            let file = args.get(2).map(String::as_str).unwrap_or("");
            match std::env::var(var) {
                Err(_) => {
                    println!("read-proc: {var} unset");
                    2
                }
                Ok(pid) => match std::fs::read(format!("/proc/{pid}/{file}")) {
                    Ok(bytes) => {
                        println!("read-proc read {} bytes", bytes.len());
                        0
                    }
                    Err(e) => {
                        println!("read-proc failed: {}", e.kind());
                        3
                    }
                },
            }
        }
        Some("alloc-touch") => {
            // Resident memory, not virtual: every page of every block is
            // written (a zeroed allocation is mapped lazily and costs nothing
            // until it is touched). Holds the blocks a few seconds so that a
            // watchdog polling the tree can see them.
            let mib: usize = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(64);
            let mut blocks: Vec<Vec<u8>> = Vec::new();
            for _ in 0..mib {
                let mut b = vec![0u8; 1024 * 1024];
                for page in (0..b.len()).step_by(4096) {
                    b[page] = 1;
                }
                std::hint::black_box(&mut b);
                blocks.push(b);
            }
            std::hint::black_box(&blocks);
            std::thread::sleep(std::time::Duration::from_secs(3));
            println!("touched {} MiB", blocks.len());
            0
        }
        Some("connect-name") => {
            // A name resolved and connected to: `host:port`.
            use std::net::ToSocketAddrs;
            let target = args.get(1).map(String::as_str).unwrap_or("localhost:443");
            match target.to_socket_addrs() {
                Err(e) => {
                    println!("resolve failed: {}", e.kind());
                    3
                }
                Ok(addrs) => {
                    let connected = addrs.into_iter().any(|a| {
                        std::net::TcpStream::connect_timeout(&a, std::time::Duration::from_secs(5))
                            .is_ok()
                    });
                    if connected {
                        println!("connected");
                        0
                    } else {
                        println!("connect failed");
                        3
                    }
                }
            }
        }
        Some("alloc") => {
            let mib: usize = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(64);
            let mut blocks: Vec<Vec<u8>> = Vec::new();
            for _ in 0..mib {
                let mut b = vec![0u8; 1024 * 1024];
                b[0] = 1;
                blocks.push(b);
            }
            println!("allocated {} MiB", blocks.len());
            0
        }
        _ => {
            eprintln!(
                "usage: mimlys-probe-tool <ok|cwd|read|write|connect|connect-name|getaddrinfo|unix-socket|unix-connect|id|sudo-id|procargs|procinfo|signal0|env|sleep|setsid-sleep|detach|fork|alloc|alloc-touch|orphan|proclist|visible|poll-time|sweeper-demo|uring-socket|uring-connect|vm-read|hold|read-proc> …"
            );
            2
        }
    };
    let _ = std::io::stdout().flush();
    std::process::exit(code);
}
