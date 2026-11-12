use super::*;

fn posix(input: &str) -> Result<WorkspacePath, PathError> {
    WorkspacePath::canonicalize_with(input, PlatformRules::Posix)
}

/// REQ-DEC-006: separators, `.` and `..` are normalized lexically; the result is relative with forward slashes.
#[test]
fn req_dec_006_lexical_normalization() {
    assert_eq!(posix("src/./lib.rs").unwrap().as_str(), "src/lib.rs");
    assert_eq!(posix("src//x/../lib.rs").unwrap().as_str(), "src/lib.rs");
    assert_eq!(posix("a\\b\\c.txt").unwrap().as_str(), "a/b/c.txt");
    assert_eq!(posix("./").unwrap(), WorkspacePath::root());
    assert_eq!(posix(".").unwrap().to_string(), ".");
    let p = posix("a/b/c").unwrap();
    assert_eq!(p.parent().unwrap().as_str(), "a/b");
    assert_eq!(p.file_name(), Some("c"));
    assert_eq!(posix("a").unwrap().parent(), Some(WorkspacePath::root()));
    assert_eq!(WorkspacePath::root().parent(), None);
}

/// REQ-WS-011: the rule set in force is the platform's. The Windows name rules themselves (reserved
/// names, forbidden characters, trailing dot or space, the drive-letter prefix, and the names that only
/// resemble them) are cases of the hostile corpus: `crates/mimlys/tests/corpus/hostile/paths/canonicalize-windows-*.json`.
#[test]
fn req_ws_011_current_rules_follow_the_platform() {
    assert_eq!(
        PlatformRules::current(),
        if cfg!(windows) {
            PlatformRules::Windows
        } else {
            PlatformRules::Posix
        }
    );
}
