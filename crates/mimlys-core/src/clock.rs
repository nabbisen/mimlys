//! Time (LIM-U3, LIM-U4): instants are absolute, in UTC, with millisecond
//! precision, rendered as RFC 3339. Time is read only through a [`Clock`]
//! so that tests and replay are deterministic (REQ-ARCH-006).

use jiff::{Timestamp, tz::TimeZone};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

/// An absolute instant with millisecond precision.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Instant(i64);

/// An instant could not be built or parsed.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum InstantError {
    /// The value is outside the representable range.
    #[error("instant out of range: {0} ms since the Unix epoch")]
    OutOfRange(i64),
    /// The text is not an RFC 3339 instant.
    #[error("not an RFC 3339 instant: {0}")]
    Parse(String),
}

impl Instant {
    /// Builds an instant from milliseconds since the Unix epoch (UTC).
    pub fn from_unix_millis(millis: i64) -> Result<Self, InstantError> {
        Timestamp::from_millisecond(millis)
            .map(|_| Self(millis))
            .map_err(|_| InstantError::OutOfRange(millis))
    }

    /// Milliseconds since the Unix epoch.
    pub fn unix_millis(&self) -> i64 {
        self.0
    }

    /// This instant moved by `millis` (negative values move it earlier).
    pub fn plus_millis(&self, millis: i64) -> Result<Self, InstantError> {
        self.0
            .checked_add(millis)
            .ok_or(InstantError::OutOfRange(i64::MAX))
            .and_then(Self::from_unix_millis)
    }

    /// Milliseconds from this instant until `later`; negative when `later` is earlier.
    pub fn millis_until(&self, later: Instant) -> i64 {
        later.0 - self.0
    }

    /// RFC 3339 in UTC with exactly three fractional digits (LIM-U4).
    pub fn to_rfc3339(&self) -> String {
        let zoned = Timestamp::from_millisecond(self.0)
            .expect("an Instant always holds a representable value")
            .to_zoned(TimeZone::UTC);
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
            zoned.year(),
            zoned.month(),
            zoned.day(),
            zoned.hour(),
            zoned.minute(),
            zoned.second(),
            zoned.millisecond()
        )
    }

    /// Parses an RFC 3339 instant; any offset is converted to UTC and
    /// sub-millisecond digits are truncated.
    pub fn parse(text: &str) -> Result<Self, InstantError> {
        let ts: Timestamp = text
            .parse()
            .map_err(|_| InstantError::Parse(text.to_owned()))?;
        Self::from_unix_millis(ts.as_millisecond())
    }
}

impl fmt::Display for Instant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_rfc3339())
    }
}

impl fmt::Debug for Instant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Instant({})", self.to_rfc3339())
    }
}

impl Serialize for Instant {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_rfc3339())
    }
}

impl<'de> Deserialize<'de> for Instant {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Instant::parse(&text).map_err(serde::de::Error::custom)
    }
}

/// A source of the current instant.
pub trait Clock: Send + Sync {
    /// The current instant.
    fn now(&self) -> Instant;
}

/// The operating-system clock. The only place in this crate that reads time.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant(Timestamp::now().as_millisecond())
    }
}

/// A clock that only moves when told to, for tests and replay.
#[cfg(any(test, feature = "test-support"))]
#[derive(Debug)]
pub struct FakeClock {
    now: std::sync::atomic::AtomicI64,
}

#[cfg(any(test, feature = "test-support"))]
impl FakeClock {
    /// A clock stopped at `start`.
    pub fn at(start: Instant) -> Self {
        Self {
            now: std::sync::atomic::AtomicI64::new(start.unix_millis()),
        }
    }

    /// Moves the clock forward by `millis`.
    pub fn advance_millis(&self, millis: i64) {
        self.now
            .fetch_add(millis, std::sync::atomic::Ordering::SeqCst);
    }

    /// Sets the clock to `instant`.
    pub fn set(&self, instant: Instant) {
        self.now
            .store(instant.unix_millis(), std::sync::atomic::Ordering::SeqCst);
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Default for FakeClock {
    /// Stopped at 2026-01-01T00:00:00.000Z.
    fn default() -> Self {
        Self {
            now: std::sync::atomic::AtomicI64::new(1_767_225_600_000),
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Clock for FakeClock {
    fn now(&self) -> Instant {
        Instant(self.now.load(std::sync::atomic::Ordering::SeqCst))
    }
}

#[cfg(test)]
mod tests;
