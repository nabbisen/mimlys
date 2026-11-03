use super::parse_duration_ms;

/// The command-line reference (`docs/src/reference/cli.md`): "Durations are
/// `30s`, `15m`, `2h`, `1d`". No requirement states the format, so none is
/// named here; a bare number is minutes and anything else is refused.
#[test]
fn durations_parse_as_the_command_line_reference_states() {
    assert_eq!(parse_duration_ms("30s").unwrap(), 30_000);
    assert_eq!(parse_duration_ms("15m").unwrap(), 900_000);
    assert_eq!(parse_duration_ms("2h").unwrap(), 7_200_000);
    assert_eq!(parse_duration_ms("1d").unwrap(), 86_400_000);
    assert_eq!(parse_duration_ms("5").unwrap(), 300_000);
    assert!(parse_duration_ms("soon").is_err());
}
