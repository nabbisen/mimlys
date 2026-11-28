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
