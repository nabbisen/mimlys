//! The decisions of the Windows permission code that need no system call
//! (RFC 011 §3, §7; REQ-CONF-008; external design §13.3, §13.5), kept pure so
//! that they are tested on every platform: the rights the container holds on
//! a directory, whether a directory's entries amount to the wanted entry, the
//! name of a directory's lock, the retry of the permission read, and whose
//! directory the data directory may be.
//!
//! Only the Windows module `windows::acl` calls this outside the tests.

#![cfg_attr(not(windows), allow(dead_code))]

/// What an entry grants.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    /// Read and execute: the toolchain and the system paths.
    Read,
    /// Read, write, execute and delete: the workspaces and the temporary
    /// directory. Never the right to change a file's permissions or owner.
    Write,
}

/// `READ_CONTROL`: read a file's security descriptor.
pub const READ_CONTROL: u32 = 0x0002_0000;
/// `DELETE`.
pub const DELETE: u32 = 0x0001_0000;
/// `WRITE_DAC`: change a file's permissions.
pub const WRITE_DAC: u32 = 0x0004_0000;
/// `WRITE_OWNER`: change a file's owner.
pub const WRITE_OWNER: u32 = 0x0008_0000;
/// `SYNCHRONIZE`.
pub const SYNCHRONIZE: u32 = 0x0010_0000;
/// `GENERIC_READ`, `GENERIC_WRITE`, `GENERIC_EXECUTE` and `GENERIC_ALL`, the
/// bits a mask holds before the system maps them to a file's rights.
pub const GENERIC_READ: u32 = 0x8000_0000;
/// See [`GENERIC_READ`].
pub const GENERIC_WRITE: u32 = 0x4000_0000;
/// See [`GENERIC_READ`].
pub const GENERIC_EXECUTE: u32 = 0x2000_0000;
/// See [`GENERIC_READ`].
pub const GENERIC_ALL: u32 = 0x1000_0000;

/// `FILE_GENERIC_READ` (winnt.h): `READ_CONTROL`, `FILE_READ_DATA` (list a
/// directory), `FILE_READ_ATTRIBUTES`, `FILE_READ_EA` and `SYNCHRONIZE`. A
/// program that opens a file with `GENERIC_READ` is checked against all of it.
pub const FILE_GENERIC_READ: u32 = 0x0012_0089;
/// `FILE_GENERIC_WRITE` (winnt.h): `READ_CONTROL`, `FILE_WRITE_DATA` (create a
/// file), `FILE_WRITE_ATTRIBUTES`, `FILE_WRITE_EA`, `FILE_APPEND_DATA` (create
/// a subdirectory) and `SYNCHRONIZE`.
pub const FILE_GENERIC_WRITE: u32 = 0x0012_0116;
/// `FILE_GENERIC_EXECUTE` (winnt.h): `READ_CONTROL`, `FILE_READ_ATTRIBUTES`,
/// `FILE_EXECUTE` (run a program, enter a directory) and `SYNCHRONIZE`.
pub const FILE_GENERIC_EXECUTE: u32 = 0x0012_00A0;
/// `FILE_ALL_ACCESS` (winnt.h): every specific right, `WRITE_DAC` and
/// `WRITE_OWNER` among them.
pub const FILE_ALL_ACCESS: u32 = 0x001F_01FF;

/// What the container holds on the workspaces and `<data>/tmp`: read, write,
/// execute and delete (`0x0013_01BF`). Each right is the one thing a build in
/// the workspace needs, and none more:
/// - `FILE_GENERIC_READ`: open and read sources and artifacts, list a
///   directory, read attributes;
/// - `FILE_GENERIC_WRITE`: create files and subdirectories, write and append,
///   set attributes (clearing a read-only attribute before replacing a file);
/// - `FILE_GENERIC_EXECUTE`: run the programs a build produces (a test binary)
///   and enter directories;
/// - `DELETE`: remove a file or directory, rename one (a rename needs `DELETE`
///   on its source), and replace one (a linker writes a new artifact over the
///   old). Every file beneath inherits the entry, so each file carries its own
///   `DELETE`; `FILE_DELETE_CHILD` on the parent is not needed.
///
/// Not among them, on purpose: `WRITE_DAC` and `WRITE_OWNER` (a tool could
/// open any task's files to another local account), and so `GENERIC_ALL` and
/// `FILE_ALL_ACCESS`, which hold both. `READ_CONTROL` (read a permission
/// list) and `SYNCHRONIZE` come with the generic file rights; neither changes
/// anything.
pub const WRITE_MASK: u32 = FILE_GENERIC_READ | FILE_GENERIC_WRITE | FILE_GENERIC_EXECUTE | DELETE;

/// What the container holds on the toolchain paths: read and execute.
pub const READ_MASK: u32 = FILE_GENERIC_READ | FILE_GENERIC_EXECUTE;

/// The mask an entry of this kind is written with. The write entry uses
/// specific rights only, so the system stores it as one entry with exactly
/// this mask (a generic right in an inheritable entry is split in two, and
/// is stored generic in the second).
pub fn wanted_mask(access: Access) -> u32 {
    match access {
        Access::Read => READ_MASK,
        Access::Write => WRITE_MASK,
    }
}

/// A mask with its generic bits mapped to the rights of a file or
/// directory, as the system does when it checks access.
pub fn map_generic(mask: u32) -> u32 {
    let mut mapped = mask & !(GENERIC_READ | GENERIC_WRITE | GENERIC_EXECUTE | GENERIC_ALL);
    if mask & GENERIC_READ != 0 {
        mapped |= FILE_GENERIC_READ;
    }
    if mask & GENERIC_WRITE != 0 {
        mapped |= FILE_GENERIC_WRITE;
    }
    if mask & GENERIC_EXECUTE != 0 {
        mapped |= FILE_GENERIC_EXECUTE;
    }
    if mask & GENERIC_ALL != 0 {
        mapped |= FILE_ALL_ACCESS;
    }
    mapped
}

/// One access-allowed entry of a directory's permissions that names the
/// container, as read back from the directory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ace {
    /// The entry's mask, as stored.
    pub mask: u32,
    /// The entry came from the parent (`INHERITED_ACE`), not from a write to
    /// this directory.
    pub inherited: bool,
    /// The entry applies only to what lies beneath (`INHERIT_ONLY_ACE`).
    pub inherit_only: bool,
}

/// What a directory's entries for the container amount to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryState {
    /// No entry of its own for the container (an inherited one does not count).
    Absent,
    /// An entry of its own exists but is not the wanted one: it lacks a wanted
    /// right, or it holds `WRITE_DAC` or `WRITE_OWNER` (the entry of 0.10 and
    /// earlier, `GENERIC_ALL`). `mask` is the union the entries hold, generic
    /// bits mapped.
    Wrong {
        /// What the entries hold together.
        mask: u32,
    },
    /// The wanted entry is in force.
    Applied,
}

/// Whether the entries of the container SID in a directory's permissions
/// are the entry `access` asks for (external design §13.3, §13.5). Only
/// entries the directory carries itself count: one inherited from a parent
/// goes with the parent's, and no `revoke` of this directory could remove it.
/// The entry must apply to this directory (not be inherit-only), hold every
/// wanted right, and hold neither `WRITE_DAC` nor `WRITE_OWNER` in any entry,
/// inherit-only ones included (they pass to what lies beneath).
pub fn classify(access: Access, aces: &[Ace]) -> EntryState {
    let own: Vec<&Ace> = aces.iter().filter(|a| !a.inherited).collect();
    if own.is_empty() {
        return EntryState::Absent;
    }
    let union = own.iter().fold(0, |m, a| m | map_generic(a.mask));
    let here = own
        .iter()
        .filter(|a| !a.inherit_only)
        .fold(0, |m, a| m | map_generic(a.mask));
    let wanted = wanted_mask(access);
    if here & wanted == wanted && union & (WRITE_DAC | WRITE_OWNER) == 0 {
        EntryState::Applied
    } else {
        EntryState::Wrong { mask: union }
    }
}

/// The name of a directory's lock: the directory's identity (the volume's
/// serial number and the file id of the directory, from a handle to the
/// directory itself), never the text of its path, so that every spelling of
/// one directory (a different case, a short name, a trailing separator, a
/// junction, a `subst` drive) takes the same lock (A-23).
pub fn lock_name(volume_serial: u64, file_id: [u8; 16]) -> String {
    let id: String = file_id.iter().map(|b| format!("{b:02x}")).collect();
    format!("Global\\mimlys-dacl-{volume_serial:016x}-{id}")
}

/// `ERROR_INSUFFICIENT_BUFFER`: what a read of a security descriptor answers
/// when the descriptor grew between the call that sized it and the call that
/// read it (a write landing on the same file), seen on the Windows job as
/// "os error 122".
pub const ERROR_INSUFFICIENT_BUFFER: u32 = 122;

/// How many times the permission read is tried when the system answers
/// `ERROR_INSUFFICIENT_BUFFER`.
pub const READ_ATTEMPTS: u32 = 8;

/// Runs `read` (it answers a system error code, 0 for success) until it
/// answers anything but `ERROR_INSUFFICIENT_BUFFER`, at most `attempts`
/// times, pausing between tries. Returns the last answer. Any other error is
/// final at once.
pub fn retry_insufficient_buffer(
    attempts: u32,
    mut pause: impl FnMut(u32),
    mut read: impl FnMut() -> u32,
) -> u32 {
    let mut answer = read();
    let mut tries = 1;
    while answer == ERROR_INSUFFICIENT_BUFFER && tries < attempts {
        pause(tries);
        answer = read();
        tries += 1;
    }
    answer
}

/// Whether a directory owned by `owner` may be the data directory of the
/// user whose token names `user` and `default_owner` (the owner the system
/// gives what the user creates: the user, or the Administrators group for an
/// elevated token). The owner of a directory can always change its
/// permissions, so a directory owned by anyone else cannot be made private
/// to the user.
pub fn owner_is_acceptable(owner: &[u8], user: &[u8], default_owner: &[u8]) -> bool {
    !owner.is_empty() && (owner == user || owner == default_owner)
}

#[cfg(test)]
mod tests;
