//! Raw-syscall helpers of the confinement clause tests (REQ-TEST-013,
//! REQ-CONF-012), behind the `probe-syscalls` feature that `mimlys/test-bins`
//! enables; never part of a release. The probe tool (`mimlys-probe-tool`)
//! calls these safe functions from inside a host tool to try what the seccomp
//! filter must deny: a socket created through `io_uring` (which the `socket`
//! rule never sees), a connection made the same way, and a read of another
//! process's memory. Each function says what it tried and what the kernel
//! answered, as an errno, so a test can tell "denied by the filter" (`EPERM`)
//! from "not possible on this kernel" (`ENOSYS`, an `io_uring` disabled by
//! the sysctl). `libc` supplies the syscall numbers and the entry points;
//! the structures below are the kernel's `io_uring` ABI, written out.
//!
//! This is the one place the `mimlys` crate's tests reach foreign-function
//! code: the `mimlys` crate forbids `unsafe` (REQ-ENG-001, REQ-CONF-010).

#![allow(unsafe_code)]

use std::net::SocketAddrV4;

const IORING_OP_CONNECT: u8 = 16;
const IORING_OP_SOCKET: u8 = 45;
const IORING_ENTER_GETEVENTS: u32 = 1;
const IORING_FEAT_SINGLE_MMAP: u32 = 1;
const IORING_OFF_SQ_RING: i64 = 0;
const IORING_OFF_CQ_RING: i64 = 0x8000000;
const IORING_OFF_SQES: i64 = 0x10000000;

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct SqringOffsets {
    head: u32,
    tail: u32,
    ring_mask: u32,
    ring_entries: u32,
    flags: u32,
    dropped: u32,
    array: u32,
    resv1: u32,
    user_addr: u64,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct CqringOffsets {
    head: u32,
    tail: u32,
    ring_mask: u32,
    ring_entries: u32,
    overflow: u32,
    cqes: u32,
    flags: u32,
    resv1: u32,
    user_addr: u64,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Params {
    sq_entries: u32,
    cq_entries: u32,
    flags: u32,
    sq_thread_cpu: u32,
    sq_thread_idle: u32,
    features: u32,
    wq_fd: u32,
    resv: [u32; 3],
    sq_off: SqringOffsets,
    cq_off: CqringOffsets,
}

/// A submission queue entry, 64 bytes.
#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Sqe {
    opcode: u8,
    flags: u8,
    ioprio: u16,
    fd: i32,
    /// `off` or `addr2`.
    off: u64,
    addr: u64,
    len: u32,
    op_flags: u32,
    user_data: u64,
    buf_index: u16,
    personality: u16,
    splice_fd_in: i32,
    addr3: u64,
    pad: u64,
}

/// A completion queue entry, 16 bytes.
#[repr(C)]
#[derive(Clone, Copy)]
struct Cqe {
    user_data: u64,
    res: i32,
    flags: u32,
}

/// A ring set up and mapped, closed and unmapped on drop.
struct Ring {
    fd: i32,
    sq: *mut u8,
    cq: *mut u8,
    sqes: *mut Sqe,
    params: Params,
    maps: Vec<(*mut libc::c_void, usize)>,
}

fn errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

impl Ring {
    /// `io_uring_setup(4)` and the three mappings. The error is the errno of
    /// the step that failed, named.
    fn new() -> Result<Self, (&'static str, i32)> {
        let mut params = Params::default();
        // `Params` mirrors the kernel's structure (`repr(C)`, zeroed); the
        // returned descriptor is owned by `Ring`.
        // SAFETY: `io_uring_setup` writes only the `Params` it is given.
        let fd =
            unsafe { libc::syscall(libc::SYS_io_uring_setup, 4u32, &mut params as *mut Params) };
        if fd < 0 {
            return Err(("io_uring_setup", errno()));
        }
        let mut ring = Ring {
            fd: fd as i32,
            sq: std::ptr::null_mut(),
            cq: std::ptr::null_mut(),
            sqes: std::ptr::null_mut(),
            params,
            maps: Vec::new(),
        };
        let mut sq_len =
            params.sq_off.array as usize + params.sq_entries as usize * std::mem::size_of::<u32>();
        let mut cq_len =
            params.cq_off.cqes as usize + params.cq_entries as usize * std::mem::size_of::<Cqe>();
        let single = params.features & IORING_FEAT_SINGLE_MMAP != 0;
        if single {
            sq_len = sq_len.max(cq_len);
            cq_len = sq_len;
        }
        ring.sq = ring.map(sq_len, IORING_OFF_SQ_RING)? as *mut u8;
        ring.cq = if single {
            ring.sq
        } else {
            ring.map(cq_len, IORING_OFF_CQ_RING)? as *mut u8
        };
        ring.sqes = ring.map(
            params.sq_entries as usize * std::mem::size_of::<Sqe>(),
            IORING_OFF_SQES,
        )? as *mut Sqe;
        Ok(ring)
    }

    fn map(&mut self, len: usize, offset: i64) -> Result<*mut libc::c_void, (&'static str, i32)> {
        // A shared mapping of the ring's own descriptor at one of the three
        // documented offsets, its length computed from the offsets the kernel
        // returned; it is unmapped in `Drop`.
        // SAFETY: a fresh mapping of our own descriptor; no existing memory is touched.
        let p = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED | libc::MAP_POPULATE,
                self.fd,
                offset,
            )
        };
        if p == libc::MAP_FAILED {
            return Err(("mmap", errno()));
        }
        self.maps.push((p, len));
        Ok(p)
    }

    /// Submits one entry and waits for its completion; the result is the
    /// completion's `res` (a value, or a negated errno).
    fn submit(&mut self, entry: Sqe) -> Result<i32, (&'static str, i32)> {
        let p = self.params;
        // The queue holds at least four entries and this function submits one
        // at a time and waits for it, so the slot at `tail & mask` is free.
        // The tail store is a release: the kernel sees the entry before the
        // new tail.
        // SAFETY: every pointer is inside a mapping made in `new` from the kernel's own offsets.
        unsafe {
            let tail_ptr = self.sq.add(p.sq_off.tail as usize) as *mut u32;
            let mask = *(self.sq.add(p.sq_off.ring_mask as usize) as *const u32);
            let array = self.sq.add(p.sq_off.array as usize) as *mut u32;
            let tail = *tail_ptr;
            let index = tail & mask;
            *self.sqes.add(index as usize) = entry;
            *array.add(index as usize) = index;
            std::sync::atomic::AtomicU32::from_ptr(tail_ptr)
                .store(tail.wrapping_add(1), std::sync::atomic::Ordering::Release);
        }
        // No signal set is passed (null, 0).
        // SAFETY: `io_uring_enter` reads only the ring submitted above.
        let entered = unsafe {
            libc::syscall(
                libc::SYS_io_uring_enter,
                self.fd,
                1u32,
                1u32,
                IORING_ENTER_GETEVENTS,
                std::ptr::null::<libc::c_void>(),
                0usize,
            )
        };
        if entered < 0 {
            return Err(("io_uring_enter", errno()));
        }
        // The head is advanced by this process alone, after the entry is read.
        // SAFETY: the completion queue lives in the mapping made in `new`.
        unsafe {
            let head_ptr = self.cq.add(p.cq_off.head as usize) as *mut u32;
            let tail_ptr = self.cq.add(p.cq_off.tail as usize) as *const u32;
            let mask = *(self.cq.add(p.cq_off.ring_mask as usize) as *const u32);
            let head = *head_ptr;
            let tail = std::sync::atomic::AtomicU32::from_ptr(tail_ptr as *mut u32)
                .load(std::sync::atomic::Ordering::Acquire);
            if head == tail {
                return Err(("io_uring completion", 0));
            }
            let cqes = self.cq.add(p.cq_off.cqes as usize) as *const Cqe;
            let res = (*cqes.add((head & mask) as usize)).res;
            *head_ptr = head.wrapping_add(1);
            Ok(res)
        }
    }
}

impl Drop for Ring {
    fn drop(&mut self) {
        for (p, len) in self.maps.drain(..) {
            // SAFETY: each pair is a mapping `map` made, and nothing else frees it.
            unsafe { libc::munmap(p, len) };
        }
        // SAFETY: the descriptor was returned by `io_uring_setup` and is closed once.
        unsafe { libc::close(self.fd) };
    }
}

fn describe(step: &str, errno: i32) -> String {
    format!(
        "{step} errno={errno} ({})",
        std::io::Error::from_raw_os_error(errno)
    )
}

/// Creates an `AF_INET` stream socket through `IORING_OP_SOCKET`. `Ok` is the
/// descriptor (closed here); `Err` names the step and the errno: `EPERM` at
/// `io_uring_setup` is the filter's answer.
pub fn uring_socket() -> Result<i32, String> {
    let mut ring = Ring::new().map_err(|(s, e)| describe(s, e))?;
    let res = ring
        .submit(Sqe {
            opcode: IORING_OP_SOCKET,
            fd: libc::AF_INET,
            off: libc::SOCK_STREAM as u64,
            ..Sqe::default()
        })
        .map_err(|(s, e)| describe(s, e))?;
    if res < 0 {
        return Err(describe("IORING_OP_SOCKET", -res));
    }
    // SAFETY: `res` is a descriptor the kernel just created for this process.
    unsafe { libc::close(res) };
    Ok(res)
}

/// Creates a socket and connects it to `addr` through `io_uring`
/// (`IORING_OP_SOCKET`, then `IORING_OP_CONNECT`). `Ok` means the kernel
/// completed the connection; the listener at `addr` then sees it.
pub fn uring_connect(addr: SocketAddrV4) -> Result<(), String> {
    let mut ring = Ring::new().map_err(|(s, e)| describe(s, e))?;
    let sock = ring
        .submit(Sqe {
            opcode: IORING_OP_SOCKET,
            fd: libc::AF_INET,
            off: libc::SOCK_STREAM as u64,
            ..Sqe::default()
        })
        .map_err(|(s, e)| describe(s, e))?;
    if sock < 0 {
        return Err(describe("IORING_OP_SOCKET", -sock));
    }
    let sockaddr = libc::sockaddr_in {
        sin_family: libc::AF_INET as libc::sa_family_t,
        sin_port: addr.port().to_be(),
        sin_addr: libc::in_addr {
            s_addr: u32::from(*addr.ip()).to_be(),
        },
        sin_zero: [0; 8],
    };
    let res = ring
        .submit(Sqe {
            opcode: IORING_OP_CONNECT,
            fd: sock,
            off: std::mem::size_of::<libc::sockaddr_in>() as u64,
            addr: &sockaddr as *const libc::sockaddr_in as u64,
            ..Sqe::default()
        })
        .map_err(|(s, e)| describe(s, e));
    // SAFETY: `sock` is the descriptor created above, closed once.
    unsafe { libc::close(sock) };
    match res? {
        0 => Ok(()),
        r => Err(describe("IORING_OP_CONNECT", -r)),
    }
}

/// `process_vm_readv` of `len` bytes at `addr` in process `pid`. The error is
/// the errno: `EPERM` is the filter's answer (or the kernel's own check for
/// a process that is not ours), `ESRCH` no such process, `EFAULT` a bad address.
pub fn vm_read(pid: i32, addr: usize, len: usize) -> Result<Vec<u8>, i32> {
    let mut buffer = vec![0u8; len];
    let local = libc::iovec {
        iov_base: buffer.as_mut_ptr() as *mut libc::c_void,
        iov_len: len,
    };
    let remote = libc::iovec {
        iov_base: addr as *mut libc::c_void,
        iov_len: len,
    };
    // One local iovec over a buffer of `len` bytes owned here, one remote
    // iovec the kernel validates against the other process.
    // SAFETY: the local buffer is `len` bytes long; nothing here is read through `remote`.
    let n = unsafe { libc::process_vm_readv(pid, &local, 1, &remote, 1, 0) };
    if n < 0 {
        return Err(errno());
    }
    buffer.truncate(n as usize);
    Ok(buffer)
}
