//! The one spelling of a directory in the record and the commands.

use super::{normalized, same_path};

/// REQ-CONF-008: one directory under two spellings is one entry (a `..`
/// segment here; the short and the long forms on Windows), and a missing
/// directory keeps its name.
#[test]
fn req_conf_008_one_spelling_per_directory() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("a")).unwrap();
    let plain = dir.path().join("a");
    let roundabout = dir.path().join("a").join("..").join("a");
    assert!(same_path(
        plain.to_str().unwrap(),
        roundabout.to_str().unwrap()
    ));
    assert_eq!(normalized(&roundabout), normalized(&plain));
    assert!(!normalized(&plain).starts_with(r"\\?\"));
    let missing = dir.path().join("missing");
    assert_eq!(normalized(&missing), missing.to_string_lossy());
    assert!(!same_path(
        plain.to_str().unwrap(),
        missing.to_str().unwrap()
    ));
}
