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

/// REQ-SBX-001 (external design §12 and §18): an interpreter download that
/// failed for good is `unavailable` with `component: "download"`, the URL and
/// the last status in the detail; the message names the `--from FILE` remedy.
/// It is typed, never recognized by its text: the same words in an ordinary
/// error are `internal`.
#[test]
fn req_sbx_001_a_failed_download_is_unavailable_download() {
    let error = AppError::Download(crate::download::DownloadError {
        url: "https://example.invalid/qjs.wasm".into(),
        language: "javascript".into(),
        last_status: Some(504),
        last: "HTTP status 504".into(),
        retries: 3,
    });
    assert_eq!(error.kind(), "unavailable");
    let value = error.to_json();
    assert_eq!(value["error"]["kind"], "unavailable");
    assert_eq!(value["error"]["detail"]["component"], "download");
    assert_eq!(value["error"]["detail"]["last_status"], 504);
    assert_eq!(
        value["error"]["detail"]["url"],
        "https://example.invalid/qjs.wasm"
    );
    let message = value["error"]["message"].as_str().unwrap();
    assert!(message.contains("--from FILE"), "{message}");
    let lookalike = AppError::Io {
        detail: error.to_string(),
    };
    assert_eq!(lookalike.kind(), "internal");
}

/// Every source of an error of the kind `unavailable`, each with the word its `detail.component`
/// must carry: the runtime's own names where it has them (the adapter whose call it contained:
/// `model_backend`, `tool_provider`), and `permissions` and `download`. A new source of the kind
/// is added here, or the test below fails (and `AppError::component` does not compile until the
/// source is decided).
fn unavailable_sources() -> Vec<(&'static str, AppError, &'static str)> {
    use mimlys_core::runtime::{MODEL_BACKEND, TOOL_PROVIDER};
    use mimlys_core::spi::{BackendError, WorkspaceError};
    let permissions = || WorkspaceError::PermissionsNotApplied {
        reason: "acl_not_applied: the permissions lock is held".into(),
    };
    let download = crate::download::DownloadError {
        url: "https://example.invalid/qjs.wasm".into(),
        language: "javascript".into(),
        last_status: Some(504),
        last: "HTTP status 504".into(),
        retries: 3,
    };
    let [held, locked] = refusals();
    vec![
        (
            "a contained panic of the model backend",
            AppError::Runtime(RuntimeError::Unavailable {
                component: MODEL_BACKEND.into(),
            }),
            "model_backend",
        ),
        (
            "a contained panic of the tool provider",
            AppError::Runtime(RuntimeError::Unavailable {
                component: TOOL_PROVIDER.into(),
            }),
            "tool_provider",
        ),
        (
            "the backend, unreachable",
            AppError::Runtime(RuntimeError::Backend(BackendError::Unavailable {
                detail: "status:503".into(),
            })),
            "model_backend",
        ),
        (
            "the backend, past its deadline",
            AppError::Runtime(RuntimeError::Backend(BackendError::Timeout)),
            "model_backend",
        ),
        (
            "the backend, cancelled",
            AppError::Runtime(RuntimeError::Backend(BackendError::Cancelled)),
            "model_backend",
        ),
        (
            "a scripted backend with nothing left",
            AppError::Runtime(RuntimeError::Backend(BackendError::ScriptExhausted)),
            "model_backend",
        ),
        (
            "the permissions of a new workspace, through the runtime",
            AppError::Runtime(RuntimeError::Workspace(permissions())),
            "permissions",
        ),
        (
            "the permissions of a workspace",
            AppError::Workspace(permissions()),
            "permissions",
        ),
        (
            "the permission lock, held",
            AppError::Permissions(held),
            "permissions",
        ),
        (
            "the permission lock, not obtainable",
            AppError::Permissions(locked),
            "permissions",
        ),
        (
            "an interpreter download that failed",
            AppError::Download(download),
            "download",
        ),
    ]
}

/// Errors of every other kind: none of them is `unavailable`, none has a component.
fn other_errors() -> Vec<AppError> {
    use mimlys_core::spi::{PersistenceError, WorkspaceError};
    vec![
        AppError::Config(
            crate::config::Config::parse("config_version = 1\n[runtime]\nno_such_key = 1\n")
                .expect_err("an unknown key is refused"),
        ),
        AppError::Persistence(PersistenceError::Storage { detail: "x".into() }),
        AppError::Io { detail: "x".into() },
        AppError::Output {
            detail: "x".into(),
            kind: "state",
        },
        AppError::TaskRef {
            detail: "x".into(),
            not_found: true,
        },
        AppError::Workspace(WorkspaceError::NotFound { what: "x".into() }),
        AppError::Runtime(RuntimeError::InvalidRequest { detail: "x".into() }),
        AppError::Runtime(RuntimeError::NotFound { what: "x".into() }),
        AppError::Runtime(RuntimeError::Denied { detail: "x".into() }),
        AppError::Runtime(RuntimeError::Conflict { detail: "x".into() }),
        AppError::Runtime(RuntimeError::Persistence(PersistenceError::Storage {
            detail: "x".into(),
        })),
        AppError::Runtime(RuntimeError::Workspace(WorkspaceError::Refused {
            reason: "x".into(),
        })),
    ]
}

/// External design §18 (handoff 004d): every error of the kind `unavailable` carries
/// `detail.component`, one stable word, for each source of the kind; no other kind has one.
#[test]
fn req_cli_004_every_unavailable_source_names_its_component() {
    for (source, error, component) in unavailable_sources() {
        assert_eq!(error.kind(), "unavailable", "{source}");
        assert_eq!(error.component().as_deref(), Some(component), "{source}");
        let json = error.to_json();
        assert_eq!(json["error"]["kind"], "unavailable", "{source}: {json}");
        assert_eq!(
            json["error"]["detail"]["component"], component,
            "{source}: {json}"
        );
    }
    for error in other_errors() {
        assert_ne!(error.kind(), "unavailable", "{error:?}");
        assert_eq!(error.component(), None, "{error:?}");
        assert!(
            error.to_json()["error"]["detail"]
                .get("component")
                .is_none(),
            "{error:?}"
        );
    }
}

/// The words are a closed set, each one a lower-case word with underscores (a stable vocabulary
/// for a script to match on).
#[test]
fn req_cli_004_the_components_are_a_small_stable_set() {
    let mut words: Vec<&str> = unavailable_sources().iter().map(|(_, _, c)| *c).collect();
    words.sort();
    words.dedup();
    assert_eq!(
        words,
        ["download", "model_backend", "permissions", "tool_provider"]
    );
    for word in words {
        assert!(
            word.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
            "{word}"
        );
    }
}

/// The detail an `unavailable` error already had stays beside the component: the download's URL
/// and last status.
#[test]
fn req_cli_004_the_component_does_not_replace_the_rest_of_the_detail() {
    let (_, download, _) = unavailable_sources().pop().unwrap();
    let detail = &download.to_json()["error"]["detail"];
    assert_eq!(detail["component"], "download");
    assert_eq!(detail["last_status"], 504);
    assert_eq!(detail["url"], "https://example.invalid/qjs.wasm");
}
