use super::*;

/// REQ-WS-013: the reflink word follows the filesystem's type: the types that
/// always can, the ones that never can, and the ones that depend.
#[test]
fn req_ws_013_reflink_follows_the_filesystem_type() {
    for yes in ["btrfs", "bcachefs", "apfs", "refs"] {
        assert_eq!(reflink_of(yes), Reflink::Yes, "{yes}");
    }
    for no in [
        "ext4", "tmpfs", "ntfs", "vfat", "exfat", "nfs4", "msdos", "hfs",
    ] {
        assert_eq!(reflink_of(no), Reflink::No, "{no}");
    }
    for depends in ["xfs", "zfs", "overlay", "fuseblk", "", "something-new"] {
        assert_eq!(reflink_of(depends), Reflink::Unknown, "{depends}");
    }
    assert_eq!(Reflink::Yes.word(), "yes");
    assert_eq!(Reflink::No.word(), "no");
    assert_eq!(Reflink::Unknown.word(), "unknown");
}

/// REQ-WS-013: the sizes the view measures are the regular files' bytes, links
/// not followed, a missing directory nothing.
#[test]
fn req_ws_013_dir_bytes_counts_regular_files_only() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("a/b")).unwrap();
    std::fs::write(dir.path().join("a/one"), [0u8; 10]).unwrap();
    std::fs::write(dir.path().join("a/b/two"), [0u8; 20]).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(dir.path().join("a"), dir.path().join("link")).unwrap();
    assert_eq!(dir_bytes(dir.path()), 30);
    assert_eq!(dir_bytes(&dir.path().join("missing")), 0);
}

/// The words for people.
#[test]
fn bytes_and_ages_are_written_for_people() {
    use crate::messages::{age_text, bytes_text};
    assert_eq!(bytes_text(0), "0 B");
    assert_eq!(bytes_text(1023), "1023 B");
    assert_eq!(bytes_text(1024), "1.0 KiB");
    assert_eq!(bytes_text(1536 * 1024), "1.5 MiB");
    assert_eq!(age_text(-5), "0s");
    assert_eq!(age_text(59), "59s");
    assert_eq!(age_text(60), "1m");
    assert_eq!(age_text(7200), "2h");
    assert_eq!(age_text(3 * 86_400 + 5), "3d");
}
