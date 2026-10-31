use super::*;

/// LIM-U4: instants render as RFC 3339 UTC with exactly three fractional digits.
#[test]
fn lim_u4_instant_renders_rfc3339_utc_milliseconds() {
    let i = Instant::from_unix_millis(1_767_225_600_123).unwrap();
    assert_eq!(i.to_rfc3339(), "2026-01-01T00:00:00.123Z");
    let zero = Instant::from_unix_millis(0).unwrap();
    assert_eq!(zero.to_rfc3339(), "1970-01-01T00:00:00.000Z");
}

/// LIM-U4: parsing accepts offsets and extra precision and normalizes to UTC milliseconds.
#[test]
fn lim_u4_instant_parses_offsets_and_truncates_sub_millisecond() {
    let a = Instant::parse("2026-01-01T09:00:00.123456+09:00").unwrap();
    assert_eq!(a.to_rfc3339(), "2026-01-01T00:00:00.123Z");
    let b = Instant::parse("2026-01-01T00:00:00Z").unwrap();
    assert_eq!(b.unix_millis(), 1_767_225_600_000);
    assert_eq!(
        Instant::parse("yesterday"),
        Err(InstantError::Parse("yesterday".into()))
    );
}

/// LIM-U4: serde round trip uses the RFC 3339 string.
#[test]
fn lim_u4_instant_serde_round_trip() {
    let i = Instant::from_unix_millis(1_767_225_600_123).unwrap();
    let json = serde_json::to_string(&i).unwrap();
    assert_eq!(json, "\"2026-01-01T00:00:00.123Z\"");
    let back: Instant = serde_json::from_str(&json).unwrap();
    assert_eq!(back, i);
}

/// LIM-U3: arithmetic in milliseconds, with range errors instead of wrapping.
#[test]
fn lim_u3_instant_arithmetic() {
    let i = Instant::from_unix_millis(1_000).unwrap();
    assert_eq!(i.plus_millis(500).unwrap().unix_millis(), 1_500);
    assert_eq!(i.millis_until(i.plus_millis(-250).unwrap()), -250);
    assert!(matches!(
        i.plus_millis(i64::MAX),
        Err(InstantError::OutOfRange(_))
    ));
}

/// REQ-ARCH-006: the fake clock moves only when told to; the system clock is monotone enough to order.
#[test]
fn req_arch_006_fake_clock_is_controllable() {
    let clock = FakeClock::default();
    let t0 = clock.now();
    assert_eq!(clock.now(), t0);
    clock.advance_millis(30);
    assert_eq!(t0.millis_until(clock.now()), 30);
    let later = t0.plus_millis(10_000).unwrap();
    clock.set(later);
    assert_eq!(clock.now(), later);
    let s1 = SystemClock.now();
    let s2 = SystemClock.now();
    assert!(s2 >= s1);
}
