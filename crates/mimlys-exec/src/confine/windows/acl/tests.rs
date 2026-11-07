use super::*;

/// REQ-CONF-008, THR-36: with the directory's real named mutex held by
/// another writer, a second writer waits the time given and is refused with
/// the typed `Refusal::Held` naming the directory (never a write); once the
/// holder lets go the lock is taken. (The wait is short here; production
/// passes LIM-O7.)
#[test]
fn req_conf_008_a_held_mutex_refuses_after_the_wait() {
    let dir = tempfile::tempdir().unwrap();
    let holder = DaclLock::acquire_within(dir.path(), 1_000).expect("the first writer takes it");
    let refused = std::thread::scope(|scope| {
        scope
            .spawn(|| DaclLock::acquire_within(dir.path(), 150).map(|_| ()))
            .join()
            .unwrap()
    });
    match refused {
        Err(Refusal::Held { dir: named }) => assert_eq!(named, dir.path().display().to_string()),
        other => panic!("expected the refusal Held, got {other:?}"),
    }
    drop(holder);
    let again = DaclLock::acquire_within(dir.path(), 1_000);
    assert!(again.is_ok(), "free again: {:?}", again.err());
}
