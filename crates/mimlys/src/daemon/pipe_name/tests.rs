use super::*;

/// ED §19.5: the name carries the pipes' prefix and twelve digits; the same user and path always
/// give the same name.
#[test]
fn ed_19_5_a_pipes_name_is_the_prefix_and_twelve_digits_and_is_stable() {
    let name = pipe_name("S-1-5-21-1-2-3-1001", Path::new(r"C:\data\mimlys"));
    assert!(name.starts_with(PREFIX), "{name}");
    let digits = &name[PREFIX.len()..];
    assert_eq!(digits.len(), 12, "{name}");
    assert!(digits.bytes().all(|b| b.is_ascii_hexdigit()), "{name}");
    assert_eq!(
        name,
        pipe_name("S-1-5-21-1-2-3-1001", Path::new(r"C:\data\mimlys"))
    );
}

/// ED §19.5: **a pipe's name is global to the machine**, so another user (another SID) with the
/// same data directory path has another pipe, and so has another directory of the same user.
#[test]
fn ed_19_5_another_user_or_another_directory_is_another_pipe() {
    let here = Path::new(r"C:\data\mimlys");
    let mine = pipe_name("S-1-5-21-1-2-3-1001", here);
    assert_ne!(mine, pipe_name("S-1-5-21-1-2-3-1002", here));
    assert_ne!(
        mine,
        pipe_name("S-1-5-21-1-2-3-1001", Path::new(r"C:\data\other"))
    );
    // The separator keeps a SID's tail and a path's head from running together.
    assert_ne!(
        pipe_name("S-1", Path::new("2x")),
        pipe_name("S-12", Path::new("x"))
    );
}
