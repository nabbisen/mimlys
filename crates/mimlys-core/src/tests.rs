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
