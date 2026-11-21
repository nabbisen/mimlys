//! The CPU part of clause C (REQ-CONF-013; RFC 013 decision 6 with its
//! Amendment 2; LIM-H7): the share of the machine's cores a host tool's tree
//! may take, converted to what each platform's primitive counts.
//!
//! The share is a whole percent of the machine's cores (`[policy.ceiling]
//! cpu_share`, default LIM-H7: 50). A tree is always granted at least one
//! core and at most all of them, so on a machine of one core every share is
//! the whole machine. Everything here is pure and decides by its arguments:
//! the number of cores is given, never read, so that a test sets it.
//!
//! - **Linux:** `cpu.max` of the `tool` cgroup, a quota of CPU time in a
//!   period (`<quota> <period>`, microseconds); a quota above the period is
//!   several cores.
//! - **Windows:** the Job Object's CPU rate, **a share of the whole machine**,
//!   in hundredths of a percent of all processors (10 000 is every core), not
//!   a number of cores; one core of four is 2 500 (M7b report §4.2).
//! - **macOS:** no share exists; a lowered nice value ([`NICE`]) is all there
//!   is, and it is not a bound (M7b report §4.3).

/// The period of a Linux CPU quota, in microseconds (the kernel's default).
pub const PERIOD_US: u64 = 100_000;

/// The nice value of a macOS worker and its tools (`setpriority`). Never the
/// background policy (`taskpolicy -b`), which starved a tree that shared the
/// machine (M7b report §4.3); a nice value only yields to other work.
pub const NICE: i32 = 10;

/// The cores of this machine as the process may use them (at least one).
pub fn machine_cores() -> u32 {
    std::thread::available_parallelism()
        .map(|n| u32::try_from(n.get()).unwrap_or(u32::MAX))
        .unwrap_or(1)
}

/// What the share grants, in thousandths of a core: `percent` of `cores`, at
/// least one core and at most all of them.
pub fn granted_millicores(cores: u32, percent: u32) -> u64 {
    let cores = u64::from(cores.max(1));
    // percent / 100 of the cores, in thousandths: cores * percent * 10.
    let wanted = cores * u64::from(percent.min(100)) * 10;
    wanted.clamp(1_000, cores * 1_000)
}

/// Linux: the content of `cpu.max` for the share: `<quota> <period>`.
pub fn cgroup_cpu_max(cores: u32, percent: u32) -> String {
    let quota = granted_millicores(cores, percent) * PERIOD_US / 1_000;
    format!("{quota} {PERIOD_US}")
}

/// Windows: the Job Object's CPU rate for the share, in hundredths of a
/// percent of **the whole machine** (1 to 10 000). Half the cores is 5 000 on
/// any machine; one core is `10 000 / cores`.
pub fn job_cpu_rate(cores: u32, percent: u32) -> u32 {
    let cores = cores.max(1);
    let rate = granted_millicores(cores, percent) * 10 / u64::from(cores);
    rate.clamp(1, 10_000) as u32
}

#[cfg(test)]
mod tests;
