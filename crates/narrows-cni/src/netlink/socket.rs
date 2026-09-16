#![cfg(target_os = "linux")]
//! Opening a `NETLINK_ROUTE` socket and sending a built message to the
//! kernel.
//!
//! [`super::message`], [`super::link`], [`super::addr`], and [`super::route`]
//! only build byte buffers. This is where a message actually reaches the
//! kernel — the crate's first `unsafe` code, restricted to the syscalls the
//! kernel's C ABI requires (AGENTS.md §3.2).
//!
//! Linux-only: `AF_NETLINK` doesn't exist on macOS, where this crate is
//! developed. `narrows-cni` still builds everywhere; this module is just
//! empty elsewhere.

use std::fmt;
use std::io;
use std::os::fd::RawFd;

use super::message::{self, NetlinkError};

/// `NLMSG_ERROR` (`include/uapi/linux/netlink.h`): the message type the
/// kernel uses for both an acknowledgement and an error report — an
/// `NLM_F_ACK` request gets exactly one of these back, with `error == 0`
/// meaning success.
const NLMSG_ERROR: u16 = 0x2;

/// The largest reply this module reads in one `recv`. The acknowledgements
/// this crate reads back are tiny (an `nlmsgerr`, plus the echoed request
/// header); this is generous headroom, not a tuned value.
const RECV_BUF_LEN: usize = 8192;

/// An open, bound `NETLINK_ROUTE` socket: links, addresses, routes, and
/// neighbours.
///
/// # Why a raw socket, not `std::net`
///
/// `std::net`'s socket types only cover `AF_INET`/`AF_INET6`. There's no
/// standard-library type for `AF_NETLINK`, so reaching it means the raw
/// `socket(2)`/`bind(2)` syscalls directly.
#[derive(Debug)]
pub struct RouteSocket {
    fd: RawFd,
}

impl RouteSocket {
    /// Opens and binds a new `NETLINK_ROUTE` socket.
    ///
    /// `nl_pid: 0` lets the kernel assign a unique port ID; nothing here
    /// needs to be addressed by a fixed one. `nl_groups: 0` means no
    /// multicast subscriptions — this socket only sends requests and reads
    /// their direct replies, never broadcast notifications.
    ///
    /// # Errors
    ///
    /// Returns the underlying `io::Error` if `socket(2)` or `bind(2)` fails.
    pub fn open() -> io::Result<Self> {
        // SAFETY: a plain three-integer syscall with no pointer arguments.
        // Failure is reported through the return value (-1 with `errno`
        // set), which the check below turns into an `io::Error` before any
        // further syscall depends on `fd` being valid.
        let fd = unsafe {
            libc::socket(
                libc::AF_NETLINK,
                libc::SOCK_RAW | libc::SOCK_CLOEXEC,
                libc::NETLINK_ROUTE,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let socket = Self { fd };

        // SAFETY: `sockaddr_nl` is plain-old-data — fixed-width integers and
        // padding, no pointers or invariants of its own — so the all-zero
        // bit pattern `mem::zeroed` produces is a valid value for it,
        // including the padding field `libc`'s binding keeps private.
        // Setting the three public fields afterward leaves it initialized.
        let mut addr: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
        #[expect(
            clippy::cast_possible_truncation,
            reason = "AF_NETLINK (16) fits in a u16 with room to spare"
        )]
        {
            addr.nl_family = libc::AF_NETLINK as libc::sa_family_t;
        }

        // SAFETY: `addr` is a valid, fully initialized `sockaddr_nl`.
        // Reinterpreting its address as `*const sockaddr` is exactly what
        // `bind(2)` expects for an `AF_NETLINK` socket, and the length
        // passed matches the struct's real size, so the kernel reads no
        // bytes past it. `socket.fd` was just returned by a successful
        // `socket(2)` call above.
        let ret = unsafe {
            libc::bind(
                socket.fd,
                std::ptr::from_ref(&addr).cast::<libc::sockaddr>(),
                size_of_sockaddr_nl(),
            )
        };
        if ret < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(socket)
    }

    /// Sends `msg` to the kernel and reads back its acknowledgement.
    ///
    /// `msg` should have been built with `NLM_F_ACK` set — every builder in
    /// [`super::link`], [`super::addr`], and [`super::route`] sets it —
    /// otherwise the kernel may have nothing to reply with.
    ///
    /// # Errors
    ///
    /// See [`AckError`] for the ways this can fail: a syscall error, a
    /// malformed reply, an unexpected reply type, or the kernel rejecting
    /// the request outright (for example `EPERM` without `CAP_NET_ADMIN`,
    /// or `EEXIST` when `NLM_F_EXCL` finds the object already there).
    pub fn send_and_ack(&self, msg: &[u8]) -> Result<(), AckError> {
        self.send(msg)?;
        self.recv_ack()
    }

    fn send(&self, msg: &[u8]) -> io::Result<()> {
        // The kernel end of a netlink socket is always addressed as pid 0,
        // whether or not `connect(2)` was ever called.
        //
        // SAFETY: `sockaddr_nl` is plain-old-data, so the all-zero bit
        // pattern `mem::zeroed` produces is a valid value for it, including
        // the padding field `libc`'s binding keeps private — the same
        // reasoning as the identical initialization in `open` above.
        let mut dest: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
        #[expect(
            clippy::cast_possible_truncation,
            reason = "AF_NETLINK (16) fits in a u16 with room to spare"
        )]
        {
            dest.nl_family = libc::AF_NETLINK as libc::sa_family_t;
        }

        // SAFETY: `msg` is a valid slice of `msg.len()` readable bytes;
        // `dest` is a fully initialized `sockaddr_nl` of the size passed.
        // `sendto` only reads through these pointers, never writes.
        let sent = unsafe {
            libc::sendto(
                self.fd,
                msg.as_ptr().cast::<libc::c_void>(),
                msg.len(),
                0,
                std::ptr::from_ref(&dest).cast::<libc::sockaddr>(),
                size_of_sockaddr_nl(),
            )
        };
        if sent < 0 {
            return Err(io::Error::last_os_error());
        }
        #[expect(clippy::cast_sign_loss, reason = "sent is checked non-negative above")]
        let sent = sent as usize;
        if sent != msg.len() {
            // A netlink datagram is delivered whole or not at all; a short
            // write here means this module handed the kernel a message a
            // buffer somewhere couldn't hold in one piece, not a partial
            // send worth retrying.
            return Err(io::Error::other(format!(
                "sendto sent {sent} of {} bytes",
                msg.len()
            )));
        }
        Ok(())
    }

    fn recv_ack(&self) -> Result<(), AckError> {
        let mut buf = [0u8; RECV_BUF_LEN];
        // SAFETY: `buf` is a valid, writable buffer of `RECV_BUF_LEN` bytes;
        // `recvfrom` writes at most that many bytes into it and returns how
        // many. The source-address output parameters are null, which
        // `recvfrom(2)` documents as valid when the caller doesn't need the
        // sender's address — this socket only ever talks to the kernel.
        let received = unsafe {
            libc::recvfrom(
                self.fd,
                buf.as_mut_ptr().cast::<libc::c_void>(),
                buf.len(),
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if received < 0 {
            return Err(AckError::Io(io::Error::last_os_error()));
        }
        #[expect(
            clippy::cast_sign_loss,
            reason = "received is checked non-negative above"
        )]
        let received = received as usize;

        let (header, payload) =
            message::parse_header(&buf[..received]).map_err(AckError::Decode)?;
        if header.msg_type != NLMSG_ERROR {
            return Err(AckError::UnexpectedReply {
                msg_type: header.msg_type,
            });
        }
        let Some(error_bytes) = payload.get(..4) else {
            return Err(AckError::TooShortForAck);
        };
        let error = i32::from_ne_bytes([
            error_bytes[0],
            error_bytes[1],
            error_bytes[2],
            error_bytes[3],
        ]);
        if error == 0 {
            Ok(())
        } else {
            Err(AckError::Rejected(io::Error::from_raw_os_error(-error)))
        }
    }
}

impl Drop for RouteSocket {
    fn drop(&mut self) {
        // SAFETY: `self.fd` was returned by the successful `socket(2)` call
        // in `open`, and `RouteSocket` is its only owner for its whole
        // lifetime — nothing else closes it, and this runs at most once.
        unsafe {
            libc::close(self.fd);
        }
    }
}

/// `size_of::<libc::sockaddr_nl>()`, as the `socklen_t` `bind`/`sendto`
/// expect. The struct is 12 bytes on every platform `libc` defines it for,
/// nowhere near `socklen_t`'s range, so this cast never loses anything.
#[expect(
    clippy::cast_possible_truncation,
    reason = "sockaddr_nl is 12 bytes; far below socklen_t's range on every real target"
)]
const fn size_of_sockaddr_nl() -> libc::socklen_t {
    size_of::<libc::sockaddr_nl>() as libc::socklen_t
}

/// Why [`RouteSocket::send_and_ack`] failed.
#[derive(Debug)]
pub enum AckError {
    /// The `sendto` or `recvfrom` syscall itself failed.
    Io(io::Error),
    /// The reply couldn't be parsed as a netlink message at all.
    Decode(NetlinkError),
    /// The reply wasn't an acknowledgement (`NLMSG_ERROR`).
    UnexpectedReply {
        /// The message type actually received.
        msg_type: u16,
    },
    /// The reply claimed to be an acknowledgement but was too short to hold
    /// one's `error` field.
    TooShortForAck,
    /// The kernel understood the request and rejected it. The wrapped error
    /// is exactly what the kernel reported (`EPERM`, `EEXIST`, `EINVAL`, …).
    Rejected(io::Error),
}

impl fmt::Display for AckError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "netlink socket I/O failed: {e}"),
            Self::Decode(e) => write!(f, "malformed netlink reply: {e}"),
            Self::UnexpectedReply { msg_type } => {
                write!(
                    f,
                    "expected an acknowledgement, got message type {msg_type}"
                )
            }
            Self::TooShortForAck => f.write_str("acknowledgement reply is too short"),
            Self::Rejected(e) => write!(f, "kernel rejected the request: {e}"),
        }
    }
}

impl std::error::Error for AckError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) | Self::Rejected(e) => Some(e),
            Self::Decode(e) => Some(e),
            Self::UnexpectedReply { .. } | Self::TooShortForAck => None,
        }
    }
}

impl From<io::Error> for AckError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::netlink::link;

    mod open {
        use super::*;

        /// Opening/binding needs no privilege; only issuing a request the
        /// kernel acts on does.
        #[test]
        fn succeeds_without_privilege() {
            let socket = RouteSocket::open();

            assert!(socket.is_ok(), "open failed: {socket:?}");
        }
    }

    mod send_and_ack {
        use super::*;

        /// Without `CAP_NET_ADMIN` (`cargo test`'s normal state), the kernel
        /// rejects a well-formed `RTM_NEWLINK` with `EPERM` — real proof the
        /// send/receive round trip and ack parsing work against a live
        /// kernel, no privilege or cleanup needed. Run privileged, this
        /// creates a real link `tests/netns/` is responsible for cleaning
        /// up, so treat that as this test's environment being wrong, not a
        /// pass.
        #[test]
        fn unprivileged_link_creation_is_rejected_with_a_clear_error() {
            let socket = RouteSocket::open().unwrap();
            let msg = link::create_veth_pair("narrows-test-probe", "narrows-test-peer", 1);

            let result = socket.send_and_ack(&msg);

            match result {
                Ok(()) => panic!(
                    "cargo test --workspace should never have CAP_NET_ADMIN; this created a \
                     real narrows-test-probe/narrows-test-peer veth pair — clean it up by hand"
                ),
                Err(AckError::Rejected(e)) => {
                    assert_eq!(
                        e.raw_os_error(),
                        Some(libc::EPERM),
                        "expected EPERM, got {e}"
                    );
                }
                Err(other) => panic!("expected Ok or an EPERM rejection, got {other}"),
            }
        }
    }
}
