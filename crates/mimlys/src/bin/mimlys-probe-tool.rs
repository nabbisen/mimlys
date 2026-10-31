//! The probe tool of the confinement tests (REQ-TEST-013): a host tool that
//! tries what its first argument says and reports in its exit code and
//! output. Built only with the `test-bins` feature; never part of a release.
//!
//! `read <path>`, `write <path>`, `connect <addr>`, `env <name>`,
//! `sleep <secs>`, `setsid-sleep <secs>`, `fork <n>`, `alloc <mib>`, `cwd`, `ok`.

use std::io::Write;

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
                "usage: mimlys-probe-tool <ok|cwd|read|write|connect|env|sleep|setsid-sleep|fork|alloc> …"
            );
            2
        }
    };
    let _ = std::io::stdout().flush();
    std::process::exit(code);
}
