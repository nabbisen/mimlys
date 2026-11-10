use super::*;

const MOUNTINFO: &str = "\
24 1 259:2 / / rw,relatime shared:1 - ext4 /dev/nvme0n1p2 rw
30 24 0:27 / /home rw,relatime shared:5 - btrfs /dev/mapper/home rw,ssd,subvolid=256
31 30 0:28 / /home/me/my\\040files rw,relatime shared:6 - xfs /dev/sdb1 rw
32 24 0:29 / /tmp rw,nosuid,nodev shared:7 - tmpfs tmpfs rw
";

/// REQ-WS-013: the filesystem of a path is the mount with the longest mount
/// point that contains it (by path components, not by text), with the kernel's
/// octal escapes undone.
#[test]
fn req_ws_013_the_filesystem_is_the_longest_mount_containing_the_path() {
    let name = |p: &str| from_mountinfo(MOUNTINFO, Path::new(p));
    assert_eq!(name("/etc/x").as_deref(), Some("ext4"));
    assert_eq!(name("/home/me/data").as_deref(), Some("btrfs"));
    assert_eq!(name("/home").as_deref(), Some("btrfs"));
    assert_eq!(name("/home/me/my files/data").as_deref(), Some("xfs"));
    assert_eq!(name("/tmp/a/b").as_deref(), Some("tmpfs"));
    // Components, not text: `/homework` is not under `/home`.
    assert_eq!(name("/homework/x").as_deref(), Some("ext4"));
}

/// REQ-WS-013: the same on macOS, from the output of `mount`.
#[test]
fn req_ws_013_the_filesystem_from_the_output_of_mount() {
    let text = "\
/dev/disk3s1s1 on / (apfs, sealed, local, read-only, journaled)
/dev/disk3s5 on /System/Volumes/Data (apfs, local, journaled, protect, root data)
/dev/disk5s1 on /Volumes/USB DRIVE (msdos, local, nodev, nosuid)
";
    let name = |p: &str| from_mount_output(text, Path::new(p));
    assert_eq!(name("/Users/me/x").as_deref(), Some("apfs"));
    assert_eq!(name("/System/Volumes/Data/Users").as_deref(), Some("apfs"));
    assert_eq!(name("/Volumes/USB DRIVE/data").as_deref(), Some("msdos"));
}

/// REQ-WS-013: on Linux the real thing agrees with `stat -f` where it exists.
#[cfg(target_os = "linux")]
#[test]
fn req_ws_013_the_filesystem_of_a_real_path_agrees_with_stat() {
    let dir = tempfile::tempdir().unwrap();
    let ours = filesystem_name(dir.path()).expect("a name on Linux");
    let Ok(out) = std::process::Command::new("stat")
        .args(["-f", "-c", "%T"])
        .arg(dir.path())
        .output()
    else {
        return;
    };
    if !out.status.success() {
        return;
    }
    let theirs = String::from_utf8_lossy(&out.stdout).trim().to_lowercase();
    // `stat` names some types by their magic (`ext2/ext3` for ext4, `overlayfs`).
    let alike = |a: &str, b: &str| a == b || b.starts_with(a) || a.starts_with(b) || b.contains(a);
    assert!(
        alike(&ours, &theirs) || (ours == "ext4" && theirs.starts_with("ext")),
        "{ours} vs stat's {theirs}"
    );
    // A path that does not exist yet is asked by its nearest existing ancestor.
    assert_eq!(
        filesystem_name(&dir.path().join("not/yet/there")).as_deref(),
        Some(ours.as_str())
    );
}
