use super::*;

fn posix(input: &str) -> Result<WorkspacePath, PathError> {
    WorkspacePath::canonicalize_with(input, PlatformRules::Posix)
}

fn windows(input: &str) -> Result<WorkspacePath, PathError> {
    WorkspacePath::canonicalize_with(input, PlatformRules::Windows)
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

/// REQ-DEC-007 / THR-03: escapes, absolute paths and drive letters are detected without touching the filesystem.
/// REQ-SEC-005 (external design §6.3), traversal: `..` that leaves the root,
/// absolute POSIX, UNC and drive-letter forms are refused, as are NUL and
/// newline in a path.
/// REQ-TEST-005 (external design §6.3), traversal and absolute forms.
/// REQ-SEC-013 (hostile corpus, repository guards §3a), hostile paths: traversal.
#[test]
fn req_dec_007_escapes_and_absolute_paths() {
    assert_eq!(posix("../x"), Err(PathError::Escapes));
    assert_eq!(posix("a/../../x"), Err(PathError::Escapes));
    assert_eq!(posix("a/b/../../../etc/shadow"), Err(PathError::Escapes));
    assert_eq!(posix("/etc/shadow"), Err(PathError::Absolute));
    assert_eq!(posix("\\\\server\\share"), Err(PathError::Absolute));
    assert_eq!(posix("C:\\x"), Err(PathError::Absolute));
    assert_eq!(posix("c:x"), Err(PathError::Absolute));
    assert_eq!(posix(""), Err(PathError::Empty));
    assert_eq!(posix("a\0b"), Err(PathError::ControlCharacter));
    assert_eq!(posix("a\nb"), Err(PathError::ControlCharacter));
}

/// REQ-WS-011: Windows naming rules are enforced under the Windows rule set and not under POSIX.
/// REQ-SEC-005 (external design §6.3), Windows path forms and reserved
/// names: `NUL`, `COM1`, `lpt9.log`, forbidden characters, trailing dot or
/// space and a drive-letter prefix are refused under the Windows rules.
/// REQ-TEST-005 (external design §6.3), Windows path forms and reserved names.
/// REQ-SEC-013 (hostile corpus, repository guards §3a), hostile paths: Windows forms.
#[test]
fn req_ws_011_windows_name_rules() {
    for bad in [
        "NUL",
        "nul.txt",
        "COM1",
        "lpt9.log",
        "a<b",
        "a?b",
        "x|y",
        "trailing.",
        "trailing ",
    ] {
        assert!(
            matches!(windows(bad), Err(PathError::InvalidName { .. })),
            "{bad}"
        );
    }
    assert_eq!(
        windows("a:b"),
        Err(PathError::Absolute),
        "a drive-letter prefix is absolute"
    );
    assert!(windows("null").is_ok());
    assert!(windows("com10").is_ok());
    assert!(windows("a.b.c").is_ok());
    assert!(posix("NUL").is_ok());
    assert!(posix("a?b").is_ok());
    assert_eq!(
        PlatformRules::current(),
        if cfg!(windows) {
            PlatformRules::Windows
        } else {
            PlatformRules::Posix
        }
    );
}

/// REQ-DEC-006: Unicode NFC applies; case modes decide identity and ancestry.
/// REQ-SEC-005 (external design §6.3), case folding: `Src/Lib.rs` and
/// `src/lib.rs` are the same path, and `SRC` an ancestor of it, under the
/// case-insensitive mode and not under the sensitive one; NFD and NFC
/// spellings of one name are equal.
/// REQ-TEST-005 (external design §6.3), case folding.
/// REQ-SEC-013 (hostile corpus, repository guards §3a), hostile paths: case and normalization.
#[test]
fn req_dec_006_nfc_and_case_modes() {
    let nfd = posix("caf\u{0065}\u{0301}.txt").unwrap();
    let nfc = posix("caf\u{00e9}.txt").unwrap();
    assert_eq!(nfd, nfc);
    let upper = posix("Src/Lib.rs").unwrap();
    let lower = posix("src/lib.rs").unwrap();
    assert!(!upper.same_as(&lower, CaseMode::Sensitive));
    assert!(upper.same_as(&lower, CaseMode::Insensitive));
    let dir = posix("SRC").unwrap();
    assert!(lower.starts_with(&dir, CaseMode::Insensitive));
    assert!(!lower.starts_with(&dir, CaseMode::Sensitive));
    assert!(lower.starts_with(&WorkspacePath::root(), CaseMode::Sensitive));
    assert!(
        !posix("srcx/a")
            .unwrap()
            .starts_with(&posix("src").unwrap(), CaseMode::Sensitive)
    );
}
