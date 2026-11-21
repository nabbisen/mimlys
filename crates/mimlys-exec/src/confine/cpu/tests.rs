use super::*;

/// REQ-CONF-013: the Linux quota is the share of the cores in a 100 ms period;
/// half the cores of 4 is two cores, a share below one core is one core, and
/// no share is above all the cores.
#[test]
fn req_conf_013_the_cgroup_quota_is_the_share_of_the_cores() {
    assert_eq!(cgroup_cpu_max(4, 50), "200000 100000");
    assert_eq!(cgroup_cpu_max(32, 50), "1600000 100000");
    // Half of three cores is a core and a half: the quota admits it.
    assert_eq!(cgroup_cpu_max(3, 50), "150000 100000");
    // "At least one core": half of one or two cores is one core.
    assert_eq!(cgroup_cpu_max(1, 50), "100000 100000");
    assert_eq!(cgroup_cpu_max(2, 50), "100000 100000");
    assert_eq!(cgroup_cpu_max(4, 1), "100000 100000");
    // "At most all the cores": 100 is every core, and more is no more.
    assert_eq!(cgroup_cpu_max(4, 100), "400000 100000");
    assert_eq!(cgroup_cpu_max(4, 400), "400000 100000");
    // A machine whose cores are not known counts as one.
    assert_eq!(cgroup_cpu_max(0, 50), "100000 100000");
}

/// REQ-CONF-013, RFC 013 Amendment 2 point 2: the Job Object's rate is a share
/// of the whole machine, so a core count is converted, never used as the rate:
/// half the cores is 5 000 on any machine and one core is `10 000 / cores`.
#[test]
fn req_conf_013_the_job_rate_is_a_share_of_the_whole_machine() {
    for cores in [2, 3, 4, 8, 16, 32, 64] {
        assert_eq!(job_cpu_rate(cores, 50), 5_000, "half of {cores} cores");
        assert_eq!(job_cpu_rate(cores, 100), 10_000, "all of {cores} cores");
    }
    // One core of four is 2 500; of eight 1 250: the share below one core is one core.
    assert_eq!(job_cpu_rate(4, 25), 2_500);
    assert_eq!(job_cpu_rate(4, 1), 2_500);
    assert_eq!(job_cpu_rate(8, 1), 1_250);
    // A machine of one core has one share only: the whole machine.
    assert_eq!(job_cpu_rate(1, 50), 10_000);
    assert_eq!(job_cpu_rate(1, 1), 10_000);
    // The rate is never a core count: 2 of 4 cores is not the rate 2.
    assert_ne!(job_cpu_rate(4, 50), 2);
    // Never outside the range the system accepts.
    assert_eq!(job_cpu_rate(4, 400), 10_000);
    assert!((1..=10_000).contains(&job_cpu_rate(0, 50)));
    assert!((1..=10_000).contains(&job_cpu_rate(1024, 1)));
}

/// The two conversions grant the same share of the machine.
#[test]
fn req_conf_013_the_two_conversions_agree_on_the_share() {
    for cores in 1..=64u32 {
        for percent in [1, 25, 33, 50, 75, 99, 100] {
            let quota: u64 = cgroup_cpu_max(cores, percent)
                .split(' ')
                .next()
                .unwrap()
                .parse()
                .unwrap();
            // Quota as a share of all the cores, in hundredths of a percent.
            let linux = quota * 10_000 / (u64::from(cores) * PERIOD_US);
            let windows = u64::from(job_cpu_rate(cores, percent));
            assert!(
                linux.abs_diff(windows) <= 1,
                "{percent} % of {cores} cores: Linux {linux}, Windows {windows}"
            );
        }
    }
}
