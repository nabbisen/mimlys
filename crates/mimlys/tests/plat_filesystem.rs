//! The filesystem clauses of the platform requirement PLAT-004 (handoff 002d
//! item 9), on the real filesystem of each platform the CI runs: the
//! workspace's symbolic links (recreated only when their target stays inside
//! the scope; on Windows the privilege to create a link is the runner's, and
//! a runner without it skips through `common::skip`, which the CI counts) and
//! the case-sensitivity probe (the mode the workspace records is the mode of
//! the filesystem it sits on, found by a probe that is not the product's, and
//! the exclusion patterns follow it). The requirement stays listed in
//! `scripts/req-test-map.txt`'s report until every clause is tested on every
//! platform, so these tests do not carry its identifier.

mod common;

use mimlys::workspace::LocalWorkspaces;
use mimlys_core::ids::{ScriptedIds, TaskId};
use mimlys_core::path::CaseMode;
use mimlys_core::policy::Policy;
use mimlys_core::spi::WorkspaceProvider;
use mimlys_core::task::ScopeSpec;
use std::path::{Path, PathBuf};

fn patterns() -> Vec<String> {
    let p = Policy::shipped();
    p.exclusions
        .materialize
        .iter()
        .chain(p.exclusions.secrets.iter())
        .cloned()
        .collect()
}

fn provider(data: &Path) -> LocalWorkspaces {
    LocalWorkspaces::new(data.to_path_buf(), patterns(), 10_000, 100).unwrap()
}

fn scope_spec(root: &Path) -> ScopeSpec {
    ScopeSpec {
        root: root.to_string_lossy().into_owned(),
        include: vec![],
    }
}

/// A symbolic link at `link` to `target`; `dir` says whether it names a
/// directory (Windows distinguishes the two kinds).
fn make_link(target: &Path, link: &Path, dir: bool) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let _ = dir;
        std::os::unix::fs::symlink(target, link)
    }
    #[cfg(windows)]
    {
        if dir {
            std::os::windows::fs::symlink_dir(target, link)
        } else {
            std::os::windows::fs::symlink_file(target, link)
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (target, link, dir);
        Err(std::io::ErrorKind::Unsupported.into())
    }
}

/// An absolute path that exists on this platform and is outside any scope.
fn absolute_target() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(r"C:\Windows\win.ini")
    } else {
        PathBuf::from("/etc/passwd")
    }
}

/// PLAT-004, symbolic-link privileges (external design §14.1, REQ-WS-002): a
/// link to a file and a link to a directory that stay inside the scope are
/// recreated in the workspace as links, with the target they had, and
/// resolve there; a link whose target leaves the scope (relative, or
/// absolute) is omitted and counted; no link is counted as a file. Where the
/// platform refuses the test the link (Windows without the privilege), the
/// test is skipped and the skip is counted.
#[test]
fn plat_004_workspace_symbolic_links_follow_the_rule_on_this_platform() {
    let name = "plat_004_workspace_symbolic_links_follow_the_rule_on_this_platform";
    let scope = tempfile::tempdir().unwrap();
    let root = scope.path();
    std::fs::create_dir_all(root.join("src/deep")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "fn a() {}\n").unwrap();
    std::fs::write(root.join("src/deep/x.txt"), "xx").unwrap();
    let up = Path::new("..").join("lib.rs");
    let leaving = Path::new("..").join("..").join("outside");
    if let Err(e) = make_link(Path::new("lib.rs"), &root.join("src/inside.rs"), false) {
        common::skip(
            name,
            &format!("this platform or runner cannot create a symbolic link: {e}"),
        );
        return;
    }
    make_link(&up, &root.join("src/deep/up.rs"), false).unwrap();
    make_link(Path::new("deep"), &root.join("src/dirlink"), true).unwrap();
    make_link(&leaving, &root.join("src/escape"), false).unwrap();
    make_link(&absolute_target(), &root.join("src/absolute"), false).unwrap();

    let data = tempfile::tempdir().unwrap();
    let task = TaskId::new(&ScriptedIds::default());
    let p = provider(data.path());
    let info = p.materialize(task, &scope_spec(root)).unwrap();
    let tree = p.tree_path(task);

    for (link, target) in [
        ("src/inside.rs", Path::new("lib.rs")),
        ("src/deep/up.rs", up.as_path()),
        ("src/dirlink", Path::new("deep")),
    ] {
        let at = tree.join(link);
        let meta = at
            .symlink_metadata()
            .unwrap_or_else(|e| panic!("{link} was not recreated: {e}"));
        assert!(meta.file_type().is_symlink(), "{link} is a link");
        assert_eq!(std::fs::read_link(&at).unwrap(), target, "{link}'s target");
    }
    assert_eq!(
        std::fs::read_to_string(tree.join("src/inside.rs")).unwrap(),
        "fn a() {}\n",
        "the file link resolves in the workspace"
    );
    assert_eq!(
        std::fs::read_to_string(tree.join("src/deep/up.rs")).unwrap(),
        "fn a() {}\n"
    );
    assert!(
        std::fs::metadata(tree.join("src/dirlink"))
            .unwrap()
            .is_dir(),
        "the directory link resolves to a directory"
    );
    assert_eq!(
        std::fs::read_to_string(tree.join("src/dirlink/x.txt")).unwrap(),
        "xx"
    );
    for omitted in ["src/escape", "src/absolute"] {
        assert!(
            tree.join(omitted).symlink_metadata().is_err(),
            "{omitted} leaves the scope and is not recreated"
        );
    }
    assert_eq!(info.symlinks_omitted, 2);
    assert_eq!(info.files, 2, "links are not counted as files");
}

/// PLAT-004, case-insensitive filesystems (REQ-WS-011): the case mode the
/// workspace records is that of the filesystem it sits on (a probe of this
/// test's own, with two spellings of one name), and the exclusion patterns
/// follow it: in a case-insensitive workspace `.ENV`, `SERVER.PEM`,
/// `Node_Modules/` and `Target/` are as excluded as their lower-case
/// spellings; in a case-sensitive one they are different names and are copied.
#[test]
fn plat_004_the_case_probe_reads_the_real_filesystem() {
    let scope = tempfile::tempdir().unwrap();
    let root = scope.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "fn a() {}\n").unwrap();
    std::fs::write(root.join(".ENV"), "SECRET=1").unwrap();
    std::fs::write(root.join("SERVER.PEM"), "pem").unwrap();
    std::fs::create_dir_all(root.join("Node_Modules/m")).unwrap();
    std::fs::write(root.join("Node_Modules/m/index.js"), "x").unwrap();
    std::fs::create_dir_all(root.join("Target/debug")).unwrap();
    std::fs::write(root.join("Target/debug/big"), "b").unwrap();

    let data = tempfile::tempdir().unwrap();
    let task = TaskId::new(&ScriptedIds::default());
    let p = provider(data.path());
    let info = p.materialize(task, &scope_spec(root)).unwrap();
    let tree = p.tree_path(task);

    // The independent probe: one name written, asked for by another spelling.
    std::fs::write(tree.join("CaseProbe.tmp"), b"").unwrap();
    let insensitive = tree.join("caseprobe.TMP").exists();
    std::fs::remove_file(tree.join("CaseProbe.tmp")).unwrap();
    assert_eq!(
        info.case_mode,
        if insensitive {
            CaseMode::Insensitive
        } else {
            CaseMode::Sensitive
        },
        "the recorded case mode is the filesystem's"
    );

    assert!(tree.join("src/lib.rs").is_file());
    for name in [
        ".ENV",
        "SERVER.PEM",
        "Node_Modules/m/index.js",
        "Target/debug/big",
    ] {
        assert_eq!(
            tree.join(name).exists(),
            !insensitive,
            "{name}: excluded exactly where the filesystem folds case"
        );
    }
    assert_eq!(info.files, if insensitive { 1 } else { 5 });
}
