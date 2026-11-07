//! Crate-wide guards.

use std::path::Path;

/// REQ-ARCH-006: time and identifiers are read only through `Clock` and
/// `IdSource`; no other source file calls the system clock or generates a UUID.
#[test]
fn req_arch_006_no_direct_time_or_uuid_calls_outside_clock_and_ids() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let forbidden = [
        "Timestamp::now",
        "SystemTime::now",
        "Instant::now()",
        "now_v7",
        "new_v4",
        "Uuid::new_v7",
    ];
    let mut offenders = Vec::new();
    visit(&src, &mut |path, text| {
        let name = path
            .strip_prefix(&src)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if name == "clock.rs" || name == "ids.rs" || name == "tests.rs" {
            return;
        }
        for needle in forbidden {
            if text.contains(needle) {
                offenders.push(format!("{name}: {needle}"));
            }
        }
    });
    assert!(
        offenders.is_empty(),
        "direct time or uuid calls: {offenders:?}"
    );
}

fn visit(dir: &Path, f: &mut dyn FnMut(&Path, &str)) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            visit(&path, f);
        } else if path.extension().is_some_and(|e| e == "rs") {
            f(&path, &std::fs::read_to_string(&path).unwrap());
        }
    }
}

/// REQ-ENG-004 (external design §21): release builds unwind, which is what
/// lets the runtime contain an adapter's panic. No profile of the workspace
/// sets `panic`, and no cargo configuration in the repository sets a
/// `-C panic` flag (the containment itself is shown by the REQ-SEC-011 tests
/// of the runtime).
#[test]
fn req_eng_004_the_workspace_does_not_abort_on_panic() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let manifest = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
    for line in manifest.lines().map(str::trim) {
        assert!(
            !(line.starts_with("panic") && line.contains('=')),
            "a profile sets `panic`: {line}"
        );
    }
    for config in [".cargo/config.toml", ".cargo/config"] {
        if let Ok(text) = std::fs::read_to_string(root.join(config)) {
            assert!(!text.contains("panic"), "{config} mentions panic");
        }
    }
    for member in std::fs::read_dir(root.join("crates")).unwrap() {
        let toml = member.unwrap().path().join("Cargo.toml");
        let text = std::fs::read_to_string(&toml).unwrap();
        assert!(
            !text
                .lines()
                .any(|l| l.trim().starts_with("panic") && l.contains('=')),
            "{} sets `panic`",
            toml.display()
        );
    }
}
