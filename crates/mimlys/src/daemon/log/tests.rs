use super::*;
use std::os::unix::fs::PermissionsExt;

/// External design §19.5, LIM-O20 and LIM-O21: the log is bounded in size and in the number of
/// files: past the bound it rotates, the copies move up, and the oldest is dropped; a line is
/// never split between two files.
#[test]
fn ed_19_5_the_log_rotates_by_size_and_keeps_a_bounded_number_of_copies() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("logs").join("daemon.log");
    let log = RotatingLog::open_bounded(&path, 100, 3).unwrap();
    // Ten lines of 30 bytes: three fit a file.
    for n in 0..10 {
        log.write_line(format!("{n:0>29}\n").as_bytes()).unwrap();
    }
    let read = |p: &Path| std::fs::read_to_string(p).unwrap();
    let names = |n: usize| {
        let mut p = path.clone().into_os_string();
        p.push(format!(".{n}"));
        PathBuf::from(p)
    };
    assert_eq!(read(&path), format!("{:0>29}\n", 9));
    assert_eq!(
        read(&names(1)),
        format!("{:0>29}\n{:0>29}\n{:0>29}\n", 6, 7, 8)
    );
    assert_eq!(
        read(&names(2)),
        format!("{:0>29}\n{:0>29}\n{:0>29}\n", 3, 4, 5)
    );
    assert_eq!(
        read(&names(3)),
        format!("{:0>29}\n{:0>29}\n{:0>29}\n", 0, 1, 2)
    );
    // One more rotation drops the oldest copy: three copies are all that stay.
    for n in 10..14 {
        log.write_line(format!("{n:0>29}\n").as_bytes()).unwrap();
    }
    assert!(!names(4).exists(), "no copy past LIM-O21");
    assert!(read(&names(3)).starts_with(&format!("{:0>29}", 3)));
    // Every file is a whole number of lines and within the bound.
    for p in [path.clone(), names(1), names(2), names(3)] {
        let text = read(&p);
        assert!(text.len() <= 100, "{}: {}", p.display(), text.len());
        assert!(text.ends_with('\n'));
    }
}

/// The log is the user's own: its directory 0700 and its file 0600.
#[test]
fn ed_19_5_the_log_is_the_users_own() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("logs").join("daemon.log");
    let _log = RotatingLog::open(&path).unwrap();
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(path.parent().unwrap()), 0o700);
    assert_eq!(mode(&path), 0o600);
}

/// `daemon status` can show the tail of the log: the last lines, whole ones.
#[test]
fn ed_19_5_the_tail_of_the_log_is_its_last_whole_lines() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("daemon.log");
    assert!(tail(&path, 5).is_empty(), "no log yet");
    std::fs::write(&path, "a\nb\nc\nd\n").unwrap();
    assert_eq!(tail(&path, 2), ["c", "d"]);
    assert_eq!(tail(&path, 10), ["a", "b", "c", "d"]);
}
