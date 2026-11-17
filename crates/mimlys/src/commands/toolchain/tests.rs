use super::revoke_report;

/// REQ-CONF-008 (handoff 011o-4, item 3): `toolchain revoke` of a path that
/// held no entry of the container says so, and the JSON says `applied: false`
/// with the reason `no_entry`; of a path that held one, it says the permission
/// was removed. (Whether the path was written is the Windows job's test of
/// `acl::revoke`.)
#[test]
fn req_conf_008_revoke_of_a_path_without_an_entry_says_it_held_none() {
    let (line, value) = revoke_report("C:\\tools", false);
    assert!(line.contains("held no permission"), "{line}");
    assert!(line.contains("C:\\tools"), "{line}");
    assert!(!line.contains("removed"), "{line}");
    assert_eq!(
        value,
        serde_json::json!({"applied": false, "reason": "no_entry", "path": "C:\\tools"})
    );
    let (line, value) = revoke_report("C:\\tools", true);
    assert_eq!(line, "permission removed from C:\\tools");
    assert_eq!(
        value,
        serde_json::json!({"applied": true, "path": "C:\\tools"})
    );
}
