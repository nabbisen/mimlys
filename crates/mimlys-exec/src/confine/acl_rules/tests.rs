use super::*;

fn ace(mask: u32) -> Ace {
    Ace {
        mask,
        inherited: false,
        inherit_only: false,
    }
}

/// REQ-CONF-008, REQ-CONF-006: the write entry is read, write, execute and
/// delete and holds neither `WRITE_DAC` nor `WRITE_OWNER` nor a generic bit;
/// the numbers are the ones winnt.h defines.
#[test]
fn req_conf_008_the_write_mask_is_read_write_execute_and_delete_only() {
    assert_eq!(WRITE_MASK, 0x0013_01BF);
    assert_eq!(READ_MASK, 0x0012_00A9);
    assert_eq!(
        WRITE_MASK & (WRITE_DAC | WRITE_OWNER),
        0,
        "a tool must not change a file's permissions or owner"
    );
    assert_eq!(
        WRITE_MASK & (GENERIC_READ | GENERIC_WRITE | GENERIC_EXECUTE | GENERIC_ALL),
        0,
        "specific rights only, so that the system stores one entry with this mask"
    );
    assert_eq!(WRITE_MASK & DELETE, DELETE);
    assert_eq!(wanted_mask(Access::Write), WRITE_MASK);
    assert_eq!(wanted_mask(Access::Read), READ_MASK);
    // The old entry held both of the rights the mask now lacks.
    assert_ne!(map_generic(GENERIC_ALL) & WRITE_DAC, 0);
    assert_ne!(map_generic(GENERIC_ALL) & WRITE_OWNER, 0);
}

/// REQ-CONF-008: the entry of 0.10 and earlier (`GENERIC_ALL`, as stored: a
/// mapped copy for the directory and a generic one that only passes on) is
/// not "applied"; neither is a mask short of a wanted right, an entry that
/// applies to what lies beneath only, nor one the directory inherits.
#[test]
fn req_conf_008_an_old_wrong_or_inherited_entry_is_not_applied() {
    // The wanted entry.
    assert_eq!(
        classify(Access::Write, &[ace(WRITE_MASK)]),
        EntryState::Applied
    );
    // The old entry: GENERIC_ALL, alone or as the system stores it (a mapped entry and an inherit-only generic one).
    assert_eq!(
        classify(Access::Write, &[ace(GENERIC_ALL)]),
        EntryState::Wrong {
            mask: FILE_ALL_ACCESS
        }
    );
    let split = [
        ace(FILE_ALL_ACCESS),
        Ace {
            mask: GENERIC_ALL,
            inherited: false,
            inherit_only: true,
        },
    ];
    assert!(matches!(
        classify(Access::Write, &split),
        EntryState::Wrong { .. }
    ));
    // A mask that lacks a wanted right (no DELETE; no execute).
    assert!(matches!(
        classify(Access::Write, &[ace(WRITE_MASK & !DELETE)]),
        EntryState::Wrong { .. }
    ));
    assert!(matches!(
        classify(
            Access::Write,
            &[ace(FILE_GENERIC_READ | FILE_GENERIC_WRITE)]
        ),
        EntryState::Wrong { .. }
    ));
    // A mask that adds WRITE_DAC to the wanted ones.
    assert!(matches!(
        classify(Access::Write, &[ace(WRITE_MASK | WRITE_DAC)]),
        EntryState::Wrong { .. }
    ));
    // The wanted mask only for what lies beneath: not in force on the directory.
    assert!(matches!(
        classify(
            Access::Write,
            &[Ace {
                mask: WRITE_MASK,
                inherited: false,
                inherit_only: true
            }]
        ),
        EntryState::Wrong { .. }
    ));
    // An inherited entry is no entry of the directory's own.
    let inherited = Ace {
        mask: WRITE_MASK,
        inherited: true,
        inherit_only: false,
    };
    assert_eq!(classify(Access::Write, &[inherited]), EntryState::Absent);
    assert_eq!(classify(Access::Write, &[]), EntryState::Absent);
    // The wanted entry beside an inherited one is applied.
    assert_eq!(
        classify(Access::Write, &[inherited, ace(WRITE_MASK)]),
        EntryState::Applied
    );
}

/// REQ-CONF-008: a read entry written with generic rights by an earlier
/// release (`GENERIC_READ | GENERIC_EXECUTE`, stored mapped or generic) is
/// still the read entry; one that holds `WRITE_DAC` is not.
#[test]
fn req_conf_008_the_read_entry_of_an_earlier_release_stays_applied() {
    let generic = GENERIC_READ | GENERIC_EXECUTE;
    assert_eq!(classify(Access::Read, &[ace(generic)]), EntryState::Applied);
    assert_eq!(
        classify(Access::Read, &[ace(READ_MASK)]),
        EntryState::Applied
    );
    // Mapped for the directory plus a generic copy that only passes on.
    let split = [
        ace(READ_MASK),
        Ace {
            mask: generic,
            inherited: false,
            inherit_only: true,
        },
    ];
    assert_eq!(classify(Access::Read, &split), EntryState::Applied);
    assert!(matches!(
        classify(Access::Read, &[ace(FILE_GENERIC_READ)]),
        EntryState::Wrong { .. }
    ));
    assert!(matches!(
        classify(Access::Read, &[ace(READ_MASK | WRITE_DAC)]),
        EntryState::Wrong { .. }
    ));
}

/// REQ-CONF-008: the lock is named by the directory's identity, not by any
/// text, so that two spellings of one directory (which have one identity)
/// share a lock and two directories do not.
#[test]
fn req_conf_008_the_lock_name_is_the_directory_identity() {
    let id = [7u8; 16];
    assert_eq!(lock_name(0xAB, id), lock_name(0xAB, id));
    assert_ne!(lock_name(0xAB, id), lock_name(0xAC, id), "another volume");
    let mut other = id;
    other[15] = 8;
    assert_ne!(lock_name(0xAB, id), lock_name(0xAB, other), "another file");
    let name = lock_name(0x1234, id);
    assert!(name.starts_with("Global\\mimlys-dacl-0000000000001234-0707"));
    assert!(name.len() < 120, "a mutex name is short: {name}");
}

/// REQ-CONF-008 (the CI flake "os error 122"): the permission read is
/// repeated while the system answers `ERROR_INSUFFICIENT_BUFFER`, gives up
/// after the attempts, and does not repeat any other answer.
#[test]
fn req_conf_008_the_read_retries_an_insufficient_buffer_and_nothing_else() {
    // 122 twice, then success.
    let mut calls = 0;
    let mut pauses = 0;
    let answer = retry_insufficient_buffer(
        READ_ATTEMPTS,
        |_| pauses += 1,
        || {
            calls += 1;
            if calls <= 2 { 122 } else { 0 }
        },
    );
    assert_eq!((answer, calls, pauses), (0, 3, 2));
    // 122 for ever: the attempts, then the last answer.
    let mut calls = 0;
    let answer = retry_insufficient_buffer(
        READ_ATTEMPTS,
        |_| {},
        || {
            calls += 1;
            122
        },
    );
    assert_eq!((answer, calls), (122, READ_ATTEMPTS));
    // Any other error is final at once.
    let mut calls = 0;
    let answer = retry_insufficient_buffer(
        READ_ATTEMPTS,
        |_| panic!("no pause for a final answer"),
        || {
            calls += 1;
            5
        },
    );
    assert_eq!((answer, calls), (5, 1));
    // Success the first time: no pause.
    assert_eq!(
        retry_insufficient_buffer(READ_ATTEMPTS, |_| panic!("no pause"), || 0),
        0
    );
}

/// REQ-WS-001: a data directory is the current user's when its owner is the
/// user or the owner the system gives the user's new objects; anyone else's
/// is refused, and an unreadable owner is not accepted.
#[test]
fn req_ws_001_a_directory_of_another_owner_is_not_acceptable() {
    let user = [1u8, 5, 0, 0, 0, 0, 0, 5, 21, 1];
    let admins = [1u8, 2, 0, 0, 0, 0, 0, 5, 32, 0, 0, 0, 32, 2, 0, 0];
    let other = [1u8, 5, 0, 0, 0, 0, 0, 5, 21, 2];
    assert!(owner_is_acceptable(&user, &user, &admins));
    assert!(owner_is_acceptable(&admins, &user, &admins));
    assert!(!owner_is_acceptable(&other, &user, &admins));
    assert!(!owner_is_acceptable(&[], &user, &admins));
}

/// REQ-CONF-008 (handoff 011o, D1): the container's name is a function of the
/// directory's identity, valid as a profile name (letters, numbers and a
/// hyphen, at most 64 characters), different for two directories and for two
/// volumes, and not the name 0.10 gave, so that the migration finds the old
/// container by it.
#[test]
fn req_conf_008_the_profile_name_follows_the_identity_not_the_spelling() {
    let id = [7u8; 16];
    let name = profile_name_for_identity(0xAB, id);
    assert_eq!(name, profile_name_for_identity(0xAB, id));
    assert!(name.starts_with("mimlys-") && name.len() == "mimlys-".len() + 16);
    assert!(name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
    let mut other = id;
    other[15] = 8;
    assert_ne!(name, profile_name_for_identity(0xAB, other));
    assert_ne!(name, profile_name_for_identity(0xAC, id));
    // 0.10's name hashed the spelling, lower-cased: two spellings that differed
    // by case were one name, and by a trailing separator two.
    assert_eq!(
        profile_name_for_spelling(r"C:\Data\Mimlys"),
        profile_name_for_spelling(r"c:\data\mimlys")
    );
    assert_ne!(
        profile_name_for_spelling(r"C:\Data\Mimlys"),
        profile_name_for_spelling("C:\\Data\\Mimlys\\")
    );
    assert_ne!(name, profile_name_for_spelling(r"C:\Data\Mimlys"));
}
