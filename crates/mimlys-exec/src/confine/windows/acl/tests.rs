use super::*;
use crate::confine::acl_rules::{GENERIC_ALL, WRITE_MASK};
use windows_sys::Win32::Security::GetSecurityDescriptorLength;

/// A SID that no one else uses: the container SID of a name made for the
/// test (derived from the name; no profile is created).
fn test_sid(name: &str) -> Vec<u8> {
    super::super::derive_sid(&format!("mimlys-test-{}-{name}", std::process::id()))
        .expect("a SID derived from a name")
}

/// What the directory's DACL holds: every access entry and how many of
/// them are inherited.
fn counts(dir: &Dir) -> (u32, u32) {
    let dacl = read_dacl(dir).expect("the DACL reads");
    let mut info = ACL_SIZE_INFORMATION {
        AceCount: 0,
        AclBytesInUse: 0,
        AclBytesFree: 0,
    };
    // SAFETY: the ACL lives in `dacl`; the out struct matches the class.
    let ok = unsafe {
        GetAclInformation(
            dacl.acl,
            (&mut info as *mut ACL_SIZE_INFORMATION).cast(),
            std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
            AclSizeInformation,
        )
    };
    assert_ne!(ok, 0);
    let mut inherited = 0;
    for index in 0..info.AceCount {
        let mut ace: *mut std::ffi::c_void = std::ptr::null_mut();
        // SAFETY: the index is below the count the system reported; the out pointer is valid.
        assert_ne!(unsafe { GetAce(dacl.acl, index, &mut ace) }, 0);
        // SAFETY: GetAce returned a pointer to an entry that starts with an ACE_HEADER.
        let header = unsafe { ace.cast::<ACE_HEADER>().read_unaligned() };
        if u32::from(header.AceFlags) & INHERITED_ACE != 0 {
            inherited += 1;
        }
    }
    (info.AceCount, inherited)
}

/// The entries of the SID on the directory, as read back.
fn entries(path: &Path, sid: &[u8]) -> Vec<Ace> {
    let dir = Dir::open(path).unwrap();
    let dacl = read_dacl(&dir).unwrap();
    entries_of(dacl.acl, sid).unwrap()
}

/// REQ-CONF-008, THR-36: with the directory's real named mutex held by
/// another writer, a second writer waits the time given and is refused with
/// the typed `Refusal::Held` naming the directory (never a write); once the
/// holder lets go the lock is taken. (The wait is short here; production
/// passes LIM-O7.)
#[test]
fn req_conf_008_a_held_mutex_refuses_after_the_wait() {
    let dir = tempfile::tempdir().unwrap();
    let opened = Dir::open(dir.path()).unwrap();
    let holder = DaclLock::acquire_within(&opened, 1_000).expect("the first writer takes it");
    let refused = std::thread::scope(|scope| {
        scope
            .spawn(|| DaclLock::acquire_within(&opened, 150).map(|_| ()))
            .join()
            .unwrap()
    });
    match refused {
        Err(Refusal::Held { dir: named }) => assert_eq!(named, dir.path().display().to_string()),
        other => panic!("expected the refusal Held, got {other:?}"),
    }
    drop(holder);
    let again = DaclLock::acquire_within(&opened, 1_000);
    assert!(again.is_ok(), "free again: {:?}", again.err());
}

/// REQ-CONF-008, A-23: every spelling of one directory takes one lock: a
/// trailing separator, another case, the verbatim prefix, a `..` detour and a
/// junction to it all name the identity of the directory, and a lock held
/// through one spelling refuses a writer who came by another; a different
/// directory is another lock.
#[test]
fn req_conf_008_every_spelling_of_a_directory_takes_one_lock() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("real");
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    let junction = root.path().join("link");
    let made = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&junction)
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        made.status.success(),
        "mklink: {}",
        String::from_utf8_lossy(&made.stdout)
    );
    let text = dir.to_string_lossy().into_owned();
    let spellings: Vec<PathBuf> = vec![
        dir.clone(),
        PathBuf::from(format!("{text}\\")),
        PathBuf::from(text.to_uppercase()),
        PathBuf::from(format!(r"\\?\{text}")),
        dir.join("sub").join(".."),
        junction.clone(),
    ];
    let names: Vec<String> = spellings
        .iter()
        .map(|p| {
            Dir::open(p)
                .unwrap_or_else(|e| panic!("{}: {e}", p.display()))
                .lock_name()
        })
        .collect();
    for (spelling, name) in spellings.iter().zip(&names) {
        assert_eq!(name, &names[0], "{} names another lock", spelling.display());
    }
    let other = Dir::open(&dir.join("sub")).unwrap().lock_name();
    assert_ne!(other, names[0], "another directory is another lock");
    // The lock held through one spelling stops a writer of another spelling.
    let holder = DaclLock::acquire_within(&Dir::open(&dir).unwrap(), 1_000).unwrap();
    let refused = std::thread::scope(|scope| {
        scope
            .spawn(|| DaclLock::acquire_within(&Dir::open(&junction).unwrap(), 150).map(|_| ()))
            .join()
            .unwrap()
    });
    assert!(
        matches!(refused, Err(Refusal::Held { .. })),
        "the junction shares the lock: {refused:?}"
    );
    drop(holder);
    // The junction is removed as a junction (its target stays).
    std::fs::remove_dir(&junction).unwrap();
    assert!(dir.join("sub").is_dir());
}

/// REQ-CONF-008, REQ-CONF-006: the write entry is exactly read, write,
/// execute and delete (one entry, `WRITE_MASK`, never `WRITE_DAC` or
/// `WRITE_OWNER`), inherited by what lies beneath with the same mask, and the
/// entry of 0.10 and earlier (`GENERIC_ALL`) is not "applied" and is replaced
/// by one write, once. The directory's inherited entries are kept.
#[test]
fn req_conf_008_the_write_entry_has_the_stated_mask_and_the_old_one_is_replaced() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("workspaces");
    std::fs::create_dir(&dir).unwrap();
    let sid = test_sid("mask");
    let opened = Dir::open(&dir).unwrap();
    let (before_total, before_inherited) = counts(&opened);
    assert_eq!(
        before_total, before_inherited,
        "a new directory has only inherited entries"
    );

    // Granted: one entry with exactly the mask, no generic bit.
    assert_eq!(
        ensure_entry(&dir, &sid, Access::Write).unwrap(),
        Ensured::Applied
    );
    let own = entries(&dir, &sid);
    assert_eq!(own.len(), 1, "{own:?}");
    assert_eq!(own[0].mask, WRITE_MASK);
    assert!(!own[0].inherited && !own[0].inherit_only);
    assert_eq!(own[0].mask & (WRITE_DAC | acl_rules::WRITE_OWNER), 0);
    assert!(has_entry(&dir, &sid, Access::Write).unwrap());
    let (total, inherited) = counts(&opened);
    assert_eq!(inherited, before_inherited, "the inherited entries stay");
    assert_eq!(total, before_total + 1);

    // A directory beneath inherits the entry with the same mask, and has no entry of its own.
    let child = dir.join("task").join("tree");
    std::fs::create_dir_all(&child).unwrap();
    let inherited_entries = entries(&child, &sid);
    assert_eq!(inherited_entries.len(), 1, "{inherited_entries:?}");
    assert!(inherited_entries[0].inherited);
    assert_eq!(inherited_entries[0].mask, WRITE_MASK);
    assert!(
        !has_entry(&child, &sid, Access::Write).unwrap(),
        "an inherited entry is not the directory's own"
    );
    assert_eq!(
        entry_state(&child, &sid, Access::Write).unwrap(),
        EntryState::Absent
    );

    // Nothing is written the second time.
    assert_eq!(
        ensure_entry(&dir, &sid, Access::Write).unwrap(),
        Ensured::AlreadyApplied
    );

    // The old entry: GENERIC_ALL, as 0.10 and earlier wrote it.
    {
        let _lock = DaclLock::acquire(&opened).unwrap();
        merge_entry(&opened, &sid, GENERIC_ALL, GRANT_ACCESS).unwrap();
    }
    assert!(
        !has_entry(&dir, &sid, Access::Write).unwrap(),
        "the old GENERIC_ALL entry is not applied"
    );
    assert!(matches!(
        entry_state(&dir, &sid, Access::Write).unwrap(),
        EntryState::Wrong { .. }
    ));
    assert!(
        has_any_entry(&dir, &sid).unwrap(),
        "revoke --all still finds it"
    );
    // The start replaces it, once.
    assert_eq!(
        ensure_entry(&dir, &sid, Access::Write).unwrap(),
        Ensured::Replaced
    );
    let own = entries(&dir, &sid);
    assert_eq!(own.len(), 1, "{own:?}");
    assert_eq!(own[0].mask, WRITE_MASK);
    assert_eq!(
        ensure_entry(&dir, &sid, Access::Write).unwrap(),
        Ensured::AlreadyApplied
    );
    // What lies beneath lost the old inherited right with it.
    let inherited_entries = entries(&child, &sid);
    assert!(
        inherited_entries.iter().all(|e| e.mask & WRITE_DAC == 0),
        "no WRITE_DAC beneath: {inherited_entries:?}"
    );

    // The user is not locked out, and revoke removes exactly the entry.
    std::fs::write(child.join("f.txt"), "x").unwrap();
    std::fs::remove_file(child.join("f.txt")).unwrap();
    revoke(&dir, &sid).unwrap();
    assert_eq!(
        entry_state(&dir, &sid, Access::Write).unwrap(),
        EntryState::Absent
    );
    assert_eq!(counts(&opened), (before_total, before_inherited));
}

/// The descriptor of the object as the system holds it, read back (the DACL
/// and what goes with it), byte for byte.
fn descriptor_bytes(dir: &Dir) -> Vec<u8> {
    let dacl = read_dacl(dir).expect("the DACL reads");
    // SAFETY: the descriptor is a valid self-relative descriptor owned by `dacl`.
    let len = unsafe { GetSecurityDescriptorLength(dacl.descriptor) } as usize;
    // SAFETY: a self-relative descriptor is one block of `len` bytes.
    unsafe { std::slice::from_raw_parts(dacl.descriptor as *const u8, len) }.to_vec()
}

/// The descriptors this thread has written so far.
fn dacl_writes() -> u32 {
    DACL_WRITES.with(|writes| writes.get())
}

/// REQ-CONF-008 (handoff 011o-4, item 3): `revoke` of a path on which the SID
/// holds no entry of its own writes nothing: the descriptor of the directory
/// and of what lies beneath it are the same bytes when read back, and no
/// descriptor was written (a rewrite makes the system walk and rewrite every
/// file beneath). An entry the directory only inherits is not its own, and is
/// left. A path that held an entry has it removed, in one write, and a second
/// `revoke` writes nothing again.
#[test]
fn req_conf_008_a_revoke_of_a_path_without_an_entry_writes_nothing() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("toolchain");
    let inner = dir.join("bin");
    std::fs::create_dir_all(&inner).unwrap();
    let file = inner.join("tool.exe");
    std::fs::write(&file, "x").unwrap();
    let sid = test_sid("noentry");
    let opened = Dir::open(&dir).unwrap();
    let opened_file = Dir::open(&file).unwrap();

    // No entry anywhere: nothing is written, nothing changes.
    let (dir_before, file_before) = (descriptor_bytes(&opened), descriptor_bytes(&opened_file));
    let writes = dacl_writes();
    assert_eq!(revoke(&dir, &sid).unwrap(), Revoked::NoEntry);
    assert_eq!(revoke(&file, &sid).unwrap(), Revoked::NoEntry);
    assert_eq!(dacl_writes(), writes, "no descriptor was written");
    assert_eq!(descriptor_bytes(&opened), dir_before);
    assert_eq!(descriptor_bytes(&opened_file), file_before);

    // An entry the path only inherits is not its own: left as it is.
    grant(&dir, &sid, Access::Read).unwrap();
    assert!(has_entry(&dir, &sid, Access::Read).unwrap());
    let inner_opened = Dir::open(&inner).unwrap();
    assert_eq!(entries(&inner, &sid).len(), 1, "inherited by the child");
    let inner_before = descriptor_bytes(&inner_opened);
    let file_before = descriptor_bytes(&opened_file);
    let writes = dacl_writes();
    assert_eq!(revoke(&inner, &sid).unwrap(), Revoked::NoEntry);
    assert_eq!(revoke(&file, &sid).unwrap(), Revoked::NoEntry);
    assert_eq!(dacl_writes(), writes, "an inherited entry is not removed");
    assert_eq!(descriptor_bytes(&inner_opened), inner_before);
    assert_eq!(descriptor_bytes(&opened_file), file_before);
    assert_eq!(entries(&inner, &sid).len(), 1, "still inherited");

    // A path with an entry of its own: removed in one write; then none again.
    let writes = dacl_writes();
    assert_eq!(revoke(&dir, &sid).unwrap(), Revoked::Removed);
    assert_eq!(dacl_writes(), writes + 1, "one write");
    assert_eq!(
        entry_state(&dir, &sid, Access::Read).unwrap(),
        EntryState::Absent
    );
    assert!(entries(&inner, &sid).is_empty(), "nor beneath it any more");
    let after = descriptor_bytes(&opened);
    let writes = dacl_writes();
    assert_eq!(revoke(&dir, &sid).unwrap(), Revoked::NoEntry);
    assert_eq!(dacl_writes(), writes, "nothing to remove the second time");
    assert_eq!(descriptor_bytes(&opened), after);
}

/// REQ-CONF-008: a read entry of an earlier release stays applied, one with
/// a wrong mask is replaced on a file as on a directory.
#[test]
fn req_conf_008_a_wrong_mask_is_not_applied_on_a_file_either() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("worker.exe");
    std::fs::write(&file, "x").unwrap();
    let sid = test_sid("file");
    ensure_entry(&file, &sid, Access::Read).unwrap();
    assert!(has_entry(&file, &sid, Access::Read).unwrap());
    // A read entry is not the write entry.
    assert!(!has_entry(&file, &sid, Access::Write).unwrap());
    let opened = Dir::open(&file).unwrap();
    {
        let _lock = DaclLock::acquire(&opened).unwrap();
        merge_entry(&opened, &sid, acl_rules::FILE_GENERIC_READ, GRANT_ACCESS).unwrap();
    }
    assert!(!has_entry(&file, &sid, Access::Read).unwrap(), "no execute");
    assert_eq!(
        ensure_entry(&file, &sid, Access::Read).unwrap(),
        Ensured::Replaced
    );
    assert!(has_entry(&file, &sid, Access::Read).unwrap());
    // A file's entry is nobody's to inherit.
    assert!(entries(&file, &sid).iter().all(|e| !e.inherit_only));
}

/// REQ-CONF-008 (the CI flake "os error 122" on `GetNamedSecurityInfoW`):
/// two threads rewrite the permissions of one file (two containers, as two
/// installations would) while a third reads them with no lock and a fourth
/// reads and decides under the lock; no call fails, and neither writer's
/// entry is lost.
#[test]
fn req_conf_008_two_writers_and_two_readers_of_one_file_never_fail() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("worker.exe");
    std::fs::write(&file, "x").unwrap();
    let (a, b, c) = (test_sid("a"), test_sid("b"), test_sid("c"));
    let done = std::sync::atomic::AtomicBool::new(false);
    std::thread::scope(|scope| {
        let writers = [
            scope.spawn(|| {
                for _ in 0..10 {
                    grant(&file, &a, Access::Write).expect("writer a");
                }
            }),
            scope.spawn(|| {
                for _ in 0..10 {
                    grant(&file, &b, Access::Read).expect("writer b");
                }
            }),
        ];
        let readers = [
            scope.spawn(|| {
                while !done.load(std::sync::atomic::Ordering::Relaxed) {
                    entry_state(&file, &a, Access::Write).expect("the lock-free read");
                    has_entry(&file, &b, Access::Read).expect("the lock-free read");
                }
            }),
            scope.spawn(|| {
                while !done.load(std::sync::atomic::Ordering::Relaxed) {
                    ensure_entry(&file, &c, Access::Read).expect("the read under the lock");
                }
            }),
        ];
        for writer in writers {
            writer.join().unwrap();
        }
        done.store(true, std::sync::atomic::Ordering::Relaxed);
        for reader in readers {
            reader.join().unwrap();
        }
    });
    for (sid, access) in [(&a, Access::Write), (&b, Access::Read), (&c, Access::Read)] {
        assert!(has_entry(&file, sid, access).unwrap(), "an entry was lost");
    }
}

/// REQ-WS-001, A-23: the data directory is created with its protected
/// descriptor (the user alone, inheritable) in the call that creates it, with
/// no moment at which it has its parent's permissions; one that exists is left
/// to the next step.
#[test]
fn req_ws_001_the_directory_is_created_with_its_protected_descriptor() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("data");
    create_protected_dir(&dir).unwrap();
    assert!(dir.is_dir());
    assert!(
        is_private_to_current_user(&dir).unwrap(),
        "created protected, before protect_to_current_user ran"
    );
    // The user can use it.
    std::fs::write(dir.join("f"), "x").unwrap();
    // An existing directory is no error and is not changed here.
    create_protected_dir(&dir).unwrap();
    // The ordinary one, made by the test, has its parent's permissions.
    let plain = root.path().join("plain");
    std::fs::create_dir(&plain).unwrap();
    assert!(!is_private_to_current_user(&plain).unwrap());
}

/// REQ-WS-001, A-23: a directory whose owner is not the current user is
/// refused, and nothing is written to it. The system directory is owned by
/// the installer account, not by the user or the Administrators group.
#[test]
fn req_ws_001_a_directory_of_another_owner_is_refused() {
    let own = tempfile::tempdir().unwrap();
    check_owner_of(own.path()).expect("a directory the user made is the user's");
    let windows = PathBuf::from(std::env::var_os("SystemRoot").expect("SystemRoot"));
    let refused = check_owner_of(&windows).expect_err("not the user's");
    assert!(refused.contains("is owned by"), "{refused}");
    assert!(
        refused.contains(&windows.display().to_string()),
        "{refused}"
    );
    let before = counts(&Dir::open(&windows).unwrap());
    let err = super::super::create_private_dir(&windows).expect_err("refused");
    assert!(err.to_string().contains("is owned by"), "{err}");
    assert_eq!(
        counts(&Dir::open(&windows).unwrap()),
        before,
        "nothing was written"
    );
}
