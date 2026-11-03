//! The probe tool of the confinement tests (REQ-TEST-013): a host tool that
//! tries what its first argument says and reports in its exit code and
//! output. Built only with the `test-bins` feature; never part of a release.
//!
//! `read <path>`, `write <path>`, `connect <addr>`, `env <name>`,
//! `sleep <secs>`, `setsid-sleep <secs>`, `fork <n>`, `alloc <mib>`, `cwd`, `ok`.
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
        #[cfg(unix)]
        Some("setsid-sleep") => {
            // A child that leaves the process group, then everyone sleeps.
            let secs = args.get(1).map(String::as_str).unwrap_or("30");
            let _ = std::process::Command::new("sh")
                .args(["-c", &format!("setsid sleep {secs} & sleep {secs}")])
                .spawn()
                .and_then(|mut c| c.wait());
            0
        }
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
                "usage: mimlys-probe-tool <ok|cwd|read|write|connect|env|sleep|setsid-sleep|fork|alloc|uring-socket|uring-connect|vm-read|hold|read-proc> …"
            );
            2
        }
    };
    let _ = std::io::stdout().flush();
    std::process::exit(code);
}
