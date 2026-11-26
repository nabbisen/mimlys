//! The CPU part of clause C (REQ-CONF-013; RFC 013 decision 6 with its
//! Amendment 2; handoff 013d item 3), through the product: the worker's own
//! report of it, `status` listing it beside the clauses, and the tier that
//! never depends on it.
//!
//! - Linux: the worker's leaf and `cpu.max` on `tool`, in a cgroup made for the
//!   test (a stand-in directory everywhere, and a real delegated cgroup when
//!   `MIMLYS_TEST_CGROUP_BASE` names one: the CI's `systemd-run -p Delegate=yes`
//!   arrangement), and `unavailable` with the reason where there is none.
//! - Windows: the Job Object's rate as the worker reads it back.
//! - macOS: the nice value of the worker, read from outside, and the word.

use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;

mod common;
use common::run_bounded;

/// A configuration, a data directory and a way to run `mimlys status`.
struct Env {
    registry: common::Registry,
    dir: tempfile::TempDir,
    config: PathBuf,
}

impl Env {
    fn new(ceiling: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("script.json");
        std::fs::write(&script, "[]").unwrap();
        let config = dir.path().join("config.toml");
        std::fs::write(
            &config,
            format!(
                "config_version = 1\n[runtime]\ndata_dir = {:?}\n[backend]\nkind = \"fake\"\nscript = {:?}\n[policy.ceiling]\n{ceiling}\n",
                dir.path().join("data").to_string_lossy(),
                script.to_string_lossy(),
            ),
        )
        .unwrap();
        Self {
            registry: common::Registry::new(),
            dir,
            config,
        }
    }

    fn command(&self, args: &[&str], env: &[(&str, &str)]) -> Command {
        let mut cmd = common::mimlys_in(&self.registry);
        cmd.arg("--config")
            .arg(&self.config)
            .args(args)
            .env("MIMLYS_CONFIG", "")
            .env("MIMLYS_WORKER_BIN", env!("CARGO_BIN_EXE_mimlys-worker"));
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd
    }

    /// `mimlys --json status`.
    fn status(&self, env: &[(&str, &str)]) -> Value {
        let out = run_bounded(
            &mut self.command(&["--json", "status"], env),
            self.dir.path(),
        );
        serde_json::from_str(out.stdout.trim())
            .unwrap_or_else(|e| panic!("not JSON ({e}): {}\n{}", out.stdout, out.stderr))
    }

    /// `mimlys status`, as the user reads it.
    fn status_text(&self, env: &[(&str, &str)]) -> String {
        run_bounded(&mut self.command(&["status"], env), self.dir.path()).stdout
    }
}

/// The line of `status` that lists the CPU part, if any.
fn cpu_line(text: &str) -> Option<&str> {
    text.lines().find(|l| l.starts_with("clause C cpu"))
}

/// REQ-CONF-013: `status` lists the CPU part beside the clauses, in the words
/// of the platform (a bound, a priority that is not one, or the reason), as a
/// line and as a field of the JSON, and the clauses are listed as before.
#[test]
fn req_conf_013_status_lists_the_cpu_part_beside_the_clauses() {
    let env = Env::new("");
    let text = env.status_text(&[]);
    let line = cpu_line(&text).unwrap_or_else(|| panic!("no CPU line in:\n{text}"));
    let status = env.status(&[]);
    let cpu = &status["cpu"];
    assert!(cpu.is_object(), "{status}");
    let state = cpu["state"].as_str().unwrap();
    // The seven clauses are still listed, each on its own line.
    for letter in ["F", "N", "E", "K", "P", "M"] {
        assert!(
            text.lines()
                .any(|l| l.starts_with(&format!("clause {letter} "))),
            "{text}"
        );
    }
    assert!(
        text.lines().any(|l| l.starts_with("clause C count")),
        "clause C itself is listed as before:\n{text}"
    );
    if cfg!(windows) {
        assert_eq!(state, "enforced", "{status}");
        assert_eq!(cpu["primitive"], "job_object");
        assert!(line.ends_with("enforced (job_object)"), "{line}");
    } else if cfg!(target_os = "macos") {
        // The word is `best_effort (priority)`, never `enforced`: a lowered
        // priority is not a bound (M7b report §4.3).
        assert_eq!(state, "best_effort", "{status}");
        assert_eq!(cpu["primitive"], "priority");
        assert!(line.ends_with("best effort (priority)"), "{line}");
    } else if state == "enforced" {
        assert_eq!(cpu["primitive"], "cgroup_cpu_max", "{status}");
        assert!(line.ends_with("enforced (cgroup_cpu_max)"), "{line}");
    } else {
        // Linux without a cgroup that passes `cpu` on: unavailable, with the reason.
        assert_eq!(state, "unavailable", "{status}");
        let reason = cpu["reason"].as_str().unwrap();
        assert!(!reason.is_empty(), "{status}");
        assert!(line.ends_with(&format!("unavailable: {reason}")), "{line}");
    }
}

/// REQ-CONF-013: `[policy.ceiling] cpu_share` out of its range is refused
/// before anything runs, naming the key and the range.
#[test]
fn req_conf_013_a_cpu_share_out_of_range_is_refused() {
    for bad in ["0", "101"] {
        let env = Env::new(&format!("cpu_share = {bad}"));
        let out = run_bounded(&mut env.command(&["status"], &[]), env.dir.path());
        assert_ne!(out.code(), 0, "{bad}: {}", out.stdout);
        let text = format!("{}{}", out.stdout, out.stderr);
        assert!(text.contains("cpu_share"), "{text}");
        assert!(text.contains("1..=100"), "{text}");
    }
    let env = Env::new("cpu_share = 25");
    assert!(env.status(&[])["cpu"].is_object());
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use mimlys_exec::confine::cpu::{cgroup_cpu_max, machine_cores};
    use std::path::Path;

    /// The cgroup stand-in: a directory that has the one file a cgroup shows.
    fn fake_base(controllers: &str) -> tempfile::TempDir {
        let base = tempfile::tempdir().unwrap();
        std::fs::write(base.path().join("cgroup.controllers"), controllers).unwrap();
        base
    }

    /// The worker's own probe over the specification the runtime gives it,
    /// with the cgroup it is made in named by `base`: its answer, and the
    /// directory it made (left in place: the runtime that removes it is not here).
    fn probe_worker(
        base: Option<&Path>,
        share: u32,
        extra: &[(&str, &str)],
    ) -> (Value, Option<PathBuf>) {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        for sub in ["tmp", "workspaces", "interpreters"] {
            std::fs::create_dir_all(data.join(sub)).unwrap();
        }
        let spec = mimlys_exec::confine::ConfinementSpec {
            cpu_share: share,
            ..mimlys::app::confinement_spec(&data, &[], &[], true)
        };
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mimlys-worker"));
        cmd.arg("--probe")
            .arg("--confine")
            .arg(serde_json::to_string(&spec).unwrap())
            .env_clear()
            .env("MIMLYS_WORKER", "1");
        if let Some(base) = base {
            cmd.env("MIMLYS_PROBE_CGROUP_BASE", base);
        }
        for (k, v) in extra {
            cmd.env(k, v);
        }
        let out = run_bounded(&mut cmd, dir.path());
        let json: Value = serde_json::from_str(out.stdout.trim())
            .unwrap_or_else(|e| panic!("not JSON ({e}): {}\n{}", out.stdout, out.stderr));
        let cgroup = json["cgroup"].as_str().map(PathBuf::from);
        (json, cgroup)
    }

    fn file(dir: &Path, name: &str) -> String {
        std::fs::read_to_string(dir.join(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
    }

    /// REQ-CONF-013, RFC 013 Amendment 2 point 1: the worker, in a cgroup made
    /// for it, stands in a leaf `worker` beside `tool`; `cpu.max` goes on `tool`,
    /// from the share and the machine's cores; the memory and process limits are
    /// on the parent, where both leaves count; and the worker reports the part
    /// `enforced`, beside the clauses.
    #[test]
    fn req_conf_013_the_worker_stands_in_a_leaf_and_tool_carries_the_quota() {
        for share in [50, 100] {
            let base = fake_base("cpu memory pids\n");
            let (json, cgroup) = probe_worker(Some(base.path()), share, &[]);
            let cgroup = cgroup.expect("the worker made its cgroup");
            assert_eq!(cgroup.parent(), Some(base.path()));
            let pid = cgroup
                .file_name()
                .unwrap()
                .to_string_lossy()
                .strip_prefix("mimlys-")
                .unwrap()
                .to_owned();
            assert_eq!(json["clauses"]["C.cpu"]["state"], "enforced", "{json}");
            assert_eq!(json["clauses"]["C.cpu"]["primitive"], "cgroup_cpu_max");
            // The worker is in its leaf; nothing is in the cgroup above.
            assert_eq!(file(&cgroup.join("worker"), "cgroup.procs"), pid);
            assert!(!cgroup.join("cgroup.procs").exists());
            // The quota is on `tool`, and on no other cgroup.
            assert_eq!(
                file(&cgroup.join("tool"), "cpu.max"),
                cgroup_cpu_max(machine_cores(), share)
            );
            assert!(!cgroup.join("cpu.max").exists());
            assert!(!cgroup.join("worker").join("cpu.max").exists());
            // The limits stay on the parent.
            assert!(cgroup.join("memory.max").exists());
            assert!(cgroup.join("pids.max").exists());
            assert!(!cgroup.join("tool").join("memory.max").exists());
            assert_eq!(json["clauses"]["M"]["state"], "enforced");
            assert_eq!(json["clauses"]["C"]["state"], "enforced");
        }
    }

    /// REQ-CONF-013: where the controller is not delegated, or there is no cgroup
    /// at all, the part is `unavailable` with the reason; the other clauses are
    /// what they were (the part is not one of them), and `cpu.max` is nowhere.
    #[test]
    fn req_conf_013_the_part_is_unavailable_with_the_reason_where_cpu_is_not_passed_on() {
        let base = fake_base("memory pids\n");
        let (json, cgroup) = probe_worker(Some(base.path()), 50, &[]);
        let cpu = &json["clauses"]["C.cpu"];
        assert_eq!(cpu["state"], "unavailable", "{json}");
        assert_eq!(cpu["reason"], "cgroup_cpu_not_delegated");
        let cgroup = cgroup.unwrap();
        assert!(!cgroup.join("tool").join("cpu.max").exists());
        assert!(!cgroup.join("cpu.max").exists());
        assert_eq!(json["clauses"]["M"]["state"], "enforced");
        assert_eq!(json["clauses"]["C"]["state"], "enforced");

        let (json, cgroup) = probe_worker(None, 50, &[("MIMLYS_PROBE_NO_CGROUP", "1")]);
        assert!(cgroup.is_none());
        let cpu = &json["clauses"]["C.cpu"];
        assert_eq!(cpu["state"], "unavailable", "{json}");
        assert!(
            cpu["reason"].as_str().unwrap().contains("injected"),
            "{json}"
        );
        // K still holds by the descendant walk, as it does without a cgroup.
        assert_eq!(json["clauses"]["K"]["state"], "enforced");
    }

    /// REQ-CONF-013: the tier does not depend on the part. Two runs that differ in
    /// that alone (the stand-in passes `cpu` on, or does not) have the same clauses
    /// and the same tier in `status`, and different CPU parts.
    #[test]
    fn req_conf_013_the_tier_is_the_same_with_and_without_the_cpu_part() {
        let env = Env::new("");
        let with = fake_base("cpu memory pids\n");
        let without = fake_base("memory pids\n");
        let a = env.status(&[("MIMLYS_PROBE_CGROUP_BASE", with.path().to_str().unwrap())]);
        let b = env.status(&[("MIMLYS_PROBE_CGROUP_BASE", without.path().to_str().unwrap())]);
        assert_eq!(a["cpu"]["state"], "enforced", "{a}");
        assert_eq!(b["cpu"]["state"], "unavailable", "{b}");
        assert_eq!(a["tier"], b["tier"]);
        assert_eq!(a["clauses"], b["clauses"]);
        assert_eq!(a["unavailable_clauses"], b["unavailable_clauses"]);
        // And the text shows both, in the platform's words.
        let text = env.status_text(&[("MIMLYS_PROBE_CGROUP_BASE", with.path().to_str().unwrap())]);
        assert!(
            cpu_line(&text)
                .unwrap()
                .ends_with("enforced (cgroup_cpu_max)"),
            "{text}"
        );
        let text =
            env.status_text(&[("MIMLYS_PROBE_CGROUP_BASE", without.path().to_str().unwrap())]);
        assert!(
            cpu_line(&text)
                .unwrap()
                .ends_with("unavailable: cgroup_cpu_not_delegated"),
            "{text}"
        );
    }

    /// REQ-CONF-013 on a real cgroup (the kernel's own answer, not a stand-in's):
    /// where `MIMLYS_TEST_CGROUP_BASE` names a delegated cgroup that holds no
    /// process and passes `cpu` on (and `memory` and `pids` if it likes), the
    /// worker's leaf and `tool`'s `cpu.max` are the kernel's, the controller is
    /// enabled for the leaves, the parent holds no process, and the limits that
    /// the cgroup offers are on the parent, where both leaves are counted.
    /// CI arranges it for one step (`systemd-run -p Delegate=yes`); elsewhere the
    /// test says so and does not count as a skip (like the opt-in engine tests).
    #[test]
    fn req_conf_013_in_a_delegated_cgroup_the_kernel_holds_the_quota_on_tool() {
        let Some(base) = std::env::var_os("MIMLYS_TEST_CGROUP_BASE").map(PathBuf::from) else {
            eprintln!("not run: MIMLYS_TEST_CGROUP_BASE names no delegated cgroup");
            return;
        };
        let offered = file(&base, "cgroup.controllers");
        let offers = |controller: &str| offered.split_whitespace().any(|c| c == controller);
        assert!(
            offers("cpu"),
            "the cgroup under test passes cpu on: {offered}"
        );
        let (json, cgroup) = probe_worker(Some(&base), 50, &[]);
        let cgroup = cgroup.expect("the worker made its cgroup");
        let report = json["clauses"].clone();
        let leaves = (cgroup.join("worker"), cgroup.join("tool"));
        let quota = file(&leaves.1, "cpu.max");
        let enabled = file(&cgroup, "cgroup.subtree_control");
        let held_by_parent = file(&cgroup, "cgroup.procs");
        let memory_max = offers("memory").then(|| file(&cgroup, "memory.max"));
        let pids_max = offers("pids").then(|| file(&cgroup, "pids.max"));
        let leaf_has_no_quota = file(&leaves.0, "cpu.max");
        // The worker has ended: its leaves are empty and go, then the cgroup.
        for dir in [&leaves.0, &leaves.1, &cgroup] {
            let _ = std::fs::remove_dir(dir);
        }
        assert_eq!(report["C.cpu"]["state"], "enforced", "{report}");
        assert_eq!(report["C.cpu"]["primitive"], "cgroup_cpu_max");
        assert_eq!(quota.trim(), cgroup_cpu_max(machine_cores(), 50));
        assert!(enabled.split_whitespace().any(|c| c == "cpu"), "{enabled}");
        assert_eq!(held_by_parent.trim(), "", "the parent holds no process");
        assert!(
            leaf_has_no_quota.starts_with("max"),
            "the worker's leaf is not bounded: {leaf_has_no_quota}"
        );
        // The limits the cgroup offers are on the parent and held; the others are
        // on their other rungs, as before.
        match memory_max {
            Some(max) => {
                assert_eq!(report["M"]["primitive"], "cgroup_memory_max", "{report}");
                assert_eq!(max.trim(), (2u64 << 30).to_string());
            }
            None => assert_eq!(report["M"]["state"], "best_effort", "{report}"),
        }
        match pids_max {
            Some(max) => {
                assert_eq!(report["C"]["primitive"], "cgroup_pids_max", "{report}");
                assert_eq!(max.trim(), "64");
            }
            None => assert_eq!(report["C"]["state"], "best_effort", "{report}"),
        }
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;

    /// REQ-CONF-013, RFC 013 Amendment 2 point 3: the worker's priority is
    /// lowered with `setpriority`, as the system itself shows it from outside (the
    /// nice value of the held probe worker), the worker reports `best_effort
    /// (priority)` and never `enforced`. The background policy is not used: it
    /// leaves the nice value as it was.
    #[test]
    fn req_conf_013_the_worker_runs_at_a_lowered_nice_value() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        for sub in ["tmp", "workspaces", "interpreters"] {
            std::fs::create_dir_all(data.join(sub)).unwrap();
        }
        let spec = mimlys::app::confinement_spec(&data, &[], &[], true);
        let mut child = Command::new(env!("CARGO_BIN_EXE_mimlys-worker"))
            .args(["--probe", "--hold", "--confine"])
            .arg(serde_json::to_string(&spec).unwrap())
            .env_clear()
            .env("MIMLYS_WORKER", "1")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        // The answer is the first line; the worker is then held alive by its input.
        let mut line = String::new();
        std::io::BufRead::read_line(
            &mut std::io::BufReader::new(child.stdout.take().unwrap()),
            &mut line,
        )
        .unwrap();
        let ps = Command::new("ps")
            .args(["-o", "nice=", "-p", &child.id().to_string()])
            .output()
            .unwrap();
        let nice = String::from_utf8_lossy(&ps.stdout).trim().to_owned();
        drop(child.stdin.take());
        let _ = child.wait();
        let json: Value =
            serde_json::from_str(line.trim()).unwrap_or_else(|e| panic!("{e}: {line}"));
        assert_eq!(
            nice,
            mimlys_exec::confine::cpu::NICE.to_string(),
            "the worker's nice value, as the system shows it"
        );
        assert_eq!(json["clauses"]["C.cpu"]["state"], "best_effort", "{json}");
        assert_eq!(json["clauses"]["C.cpu"]["primitive"], "priority");
    }
}
