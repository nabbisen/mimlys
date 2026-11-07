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

/// REQ-CONF-009: bytes on the pipe are not end of file, and while a write
/// end is open the sweeper keeps waiting (the worker is alive); only closing
/// the last write end releases it.
#[test]
fn req_conf_009_data_and_an_open_write_end_are_not_end_of_file() {
    let (reader, mut writer) = UnixStream::pair().unwrap();
    writer.write_all(b"x").unwrap();
    let fd = reader.as_raw_fd();
    let waiter = std::thread::spawn(move || end_of_file(fd));
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(!waiter.is_finished(), "a write end is open: still waiting");
    drop(writer);
    assert!(waiter.join().unwrap(), "closed: end of file");
    drop(reader);
}
