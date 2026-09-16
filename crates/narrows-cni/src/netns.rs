#![cfg(target_os = "linux")]
//! Entering another network namespace for the span of an operation, and
//! restoring the calling thread's original one afterward.
//!
//! The container runtime creates the pod's netns before execing the plugin;
//! Narrows only enters it briefly, via `setns(2)`, to build the pod's
//! interface (PLAN §6.1). This is the crate's only `setns` caller, kept in
//! its own small module per AGENTS.md §3.2.
//!
//! Linux-only. No unit tests: `setns` needs `CAP_SYS_ADMIN` even for a
//! same-namespace no-op, so there's no privilege-free way to exercise this
//! at all. Tested for real, with real privilege, in `tests/netns/`.

use std::fmt;
use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

/// Enters the network namespace at `path`, returning a guard that restores
/// the calling thread's original namespace when dropped.
///
/// `path` is typically `CNI_NETNS` — a bind-mounted path such as
/// `/run/netns/<name>` — but any path to a netns inode works, including
/// another process's `/proc/<pid>/ns/net`.
///
/// `/sys/class/net` won't confirm the switch worked; `/proc/*/ns/net` will.
/// A sysfs mount's namespace view is fixed at mount time, not read time, so
/// it can keep showing the old namespace after a successful `setns`
/// (confirmed against a real kernel, `tests/netns/`) — the same reason
/// `ip netns exec` remounts `/sys` itself. Netlink calls aren't affected;
/// they check the current namespace on every syscall.
///
/// Uses `/proc/thread-self`, not `/proc/self`: `setns` switches the calling
/// **thread**, while `/proc/self` resolves to the thread *group leader*.
/// Same thing in `narrows-cni`'s single-threaded use, but not under
/// `cargo test`, which runs tests as parallel threads.
///
/// # Errors
///
/// Returns [`EnterError`] naming which of the three fallible steps failed —
/// a bare `io::Error` alone can't (`ENOENT` doesn't say *which* open it was).
///
/// # Safety
///
/// Don't call this from a thread other code depends on staying in its
/// current namespace while the guard is held. Safe in `narrows-cni` because
/// it's single-threaded and short-lived per invocation (PLAN §4); a
/// multi-threaded caller must pin its own thread first.
pub unsafe fn enter(path: &Path) -> Result<NetNsGuard, EnterError> {
    let original = File::open("/proc/thread-self/ns/net").map_err(EnterError::OpenOwn)?;
    let target = File::open(path).map_err(|source| EnterError::OpenTarget {
        path: path.to_path_buf(),
        source,
    })?;

    // SAFETY: `target` is a freshly opened fd for a netns (or the kernel
    // reports EINVAL if it isn't one); CLONE_NEWNET is setns(2)'s documented
    // flag for switching a network namespace. Thread-pinning is the caller's
    // documented obligation, per this being an `unsafe fn`.
    let ret = unsafe { libc::setns(target.as_raw_fd(), libc::CLONE_NEWNET) };
    if ret != 0 {
        return Err(EnterError::Setns(io::Error::last_os_error()));
    }
    Ok(NetNsGuard {
        original,
        entered: path.to_path_buf(),
    })
}

/// Why [`enter`] failed.
#[derive(Debug)]
pub enum EnterError {
    /// Opening this thread's own current namespace
    /// (`/proc/thread-self/ns/net`) failed.
    OpenOwn(io::Error),
    /// Opening the target namespace failed.
    OpenTarget {
        /// The path that couldn't be opened.
        path: PathBuf,
        /// The underlying error.
        source: io::Error,
    },
    /// The `setns(2)` syscall itself failed — commonly `EPERM` without
    /// `CAP_SYS_ADMIN`, or `EINVAL` if the target wasn't a network
    /// namespace file after all.
    Setns(io::Error),
}

impl fmt::Display for EnterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OpenOwn(source) => {
                write!(
                    f,
                    "opening this thread's own namespace (/proc/thread-self/ns/net) failed: {source}"
                )
            }
            Self::OpenTarget { path, source } => {
                write!(
                    f,
                    "opening the target namespace at {} failed: {source}",
                    path.display()
                )
            }
            Self::Setns(source) => write!(f, "setns(2) failed: {source}"),
        }
    }
}

impl std::error::Error for EnterError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        let (Self::OpenOwn(source) | Self::Setns(source) | Self::OpenTarget { source, .. }) = self;
        Some(source)
    }
}

/// Restores the calling thread's original network namespace when dropped.
///
/// Returned by [`enter`]. Holding one is proof this thread switched
/// namespaces and hasn't switched back yet.
#[derive(Debug)]
pub struct NetNsGuard {
    original: File,
    /// Only for `Drop`'s error message — which namespace was being left if
    /// the restore fails.
    entered: PathBuf,
}

impl Drop for NetNsGuard {
    fn drop(&mut self) {
        // SAFETY: `self.original` is the fd `enter` opened for this thread's
        // own namespace before switching away — still open, still valid.
        let ret = unsafe { libc::setns(self.original.as_raw_fd(), libc::CLONE_NEWNET) };
        if ret != 0 {
            // Drop can't return a Result. Nothing to retry, but a thread
            // silently stuck in the wrong namespace is worth reporting.
            eprintln!(
                "narrows-cni: failed to restore the original network namespace after leaving {}: {}",
                self.entered.display(),
                io::Error::last_os_error()
            );
        }
    }
}
