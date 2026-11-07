use super::*;
use mimlys_exec::confine::dacl_wait::{Cause, Refusal};

fn refusals() -> [Refusal; 2] {
    [
        Refusal::new(Cause::Held, r"C:\tools\bin", ""),
        Refusal::new(
            Cause::LockUnavailable,
            r"C:\tools\bin",
            "Access is denied. (os error 5)",
        ),
    ]
}

/// REQ-CONF-008, external design §18: both refusals of a permission write
/// (the lock held past LIM-O7; the lock not obtainable) are the error kind
/// `unavailable` with `detail.component = "permissions"`, and the message
/// is the refusal's text, `acl_not_applied: ...`.
#[test]
fn req_conf_008_both_refusals_are_unavailable_permissions() {
    for refusal in refusals() {
        let text = refusal.to_string();
        let error = AppError::from(refusal);
        assert_eq!(error.kind(), "unavailable");
        let json = error.to_json();
        assert_eq!(json["error"]["kind"], "unavailable", "{json}");
        assert_eq!(
            json["error"]["detail"]["component"], "permissions",
            "{json}"
        );
        assert_eq!(json["error"]["message"], text, "{json}");
        assert!(text.starts_with("acl_not_applied: "), "{text}");
    }
}

/// REQ-CONF-008: the data directory's failure is recognized by the type of
/// the error it carries, never by its text: a refusal wrapped in an
/// `io::Error` is `unavailable`; the same words in another error are not;
/// any other failure carries the error's text, not only its kind.
#[test]
fn req_conf_008_data_directory_error_is_recognized_by_type() {
    let dir = Path::new("data");
    for refusal in refusals() {
        let text = refusal.to_string();
        let error = data_dir_error(dir, std::io::Error::other(refusal));
        assert!(matches!(error, AppError::Permissions(_)), "{error:?}");
        assert_eq!(error.kind(), "unavailable");
        assert_eq!(error.to_string(), text);
    }
    // The same text in a plain error is an ordinary failure.
    let lookalike = std::io::Error::other(Refusal::Held { dir: "d".into() }.to_string());
    let error = data_dir_error(dir, lookalike);
    assert!(matches!(error, AppError::Io { .. }), "{error:?}");
    assert_eq!(error.kind(), "internal");
    let denied = std::io::Error::from_raw_os_error(5);
    let text = denied.to_string();
    let error = data_dir_error(dir, denied);
    assert!(
        error.to_string().contains(&text),
        "the message carries the error's text: {error}"
    );
}
