//! The sweeper acts on end of file and on nothing else (REQ-CONF-009).

use super::end_of_file;
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;

/// REQ-CONF-009: every write end closed is end of file, and bytes before it
/// are read past; this is what the sweeper acts on.
#[test]
fn req_conf_009_end_of_file_is_every_write_end_closed() {
    let (reader, writer) = UnixStream::pair().unwrap();
    drop(writer);
    assert!(end_of_file(reader.as_raw_fd()));
    let (reader, mut writer) = UnixStream::pair().unwrap();
    writer.write_all(b"xyz").unwrap();
    drop(writer);
    assert!(end_of_file(reader.as_raw_fd()), "data, then end of file");
}

/// REQ-CONF-009: a read that fails is not end of file. A worker that is alive
/// must not lose its tools to a broken descriptor: nothing is swept.
#[test]
fn req_conf_009_a_failed_read_is_not_end_of_file() {
    assert!(!end_of_file(-1));
}
