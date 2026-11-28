use super::*;

/// LIM-O13: the idle time in force is the configured one; the test seam replaces it in a debug
/// build only.
#[test]
fn req_cli_008_the_idle_time_is_the_configured_one() {
    // The seam is a variable of the environment, which a test does not set (tests run in
    // parallel); the configured value passes through when it is not set.
    if std::env::var(IDLE_ENV).is_err() {
        assert_eq!(
            idle_in_force(Some(Duration::from_secs(60))),
            Some(Duration::from_secs(60))
        );
        assert_eq!(idle_in_force(None), None);
    }
}

/// REQ-CLI-008 (014c-5), external design §19.5: **the daemon counts client processes, by the peer's
/// pid**, and the connections they hold beside them: a process with several connections is one
/// client, and a connection that ends is no longer counted.
#[test]
fn req_cli_008_connections_are_counted_under_the_process_that_holds_them() {
    let open = Connections::default();
    assert_eq!(open.counts(), (0, 0));
    // One process: a follower's connection that stays, and a request's.
    let follower = open.open(Some(100));
    let request = open.open(Some(100));
    assert_eq!(open.counts(), (1, 2), "two connections, one client");
    // Another process.
    let other = open.open(Some(200));
    assert_eq!(open.counts(), (2, 3));
    drop(request);
    assert_eq!(
        open.counts(),
        (2, 2),
        "a connection that ended is not counted"
    );
    drop(follower);
    assert_eq!(
        open.counts(),
        (1, 1),
        "the last connection of a process ends it"
    );
    drop(other);
    assert_eq!(open.counts(), (0, 0));
}
