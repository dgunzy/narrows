//! Persisting leases across invocations.
//!
//! The plugin is a fresh process per operation, so its allocator starts empty
//! every time. Without a store, two pods would get the same address — the
//! lease table lives in a file each invocation reads, updates, and writes
//! back.
//!
//! Two failure modes shape this module. **Concurrency:** the runtime may run
//! several ADDs at once, so an exclusive lock covers the whole
//! read-modify-write. **Torn writes:** a crash mid-rewrite must not truncate
//! the file, so writes go to a temp file, are flushed, then renamed over the
//! real path — a same-directory rename is atomic.

use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::allocator::{Allocator, IpamError, LeaseKey};
use super::cidr::Ipv4Cidr;
use crate::error::{CniError, ErrorCode};

/// The on-disk format version.
///
/// Every persisted structure carries one from the start, so a later change
/// can migrate old files instead of guessing (PLAN §14.7).
pub const FORMAT_VERSION: u32 = 1;

/// Why a store operation failed.
#[derive(Debug)]
pub enum StoreError {
    /// The state file couldn't be opened, locked, read, or replaced.
    Io {
        /// What was being attempted.
        action: &'static str,
        /// The path involved.
        path: PathBuf,
        /// The underlying error.
        source: std::io::Error,
    },
    /// The file exists but isn't valid state.
    Corrupt {
        /// The state file's path.
        path: PathBuf,
        /// What was wrong with it.
        reason: String,
    },
    /// The state couldn't be turned back into an allocator.
    Ipam(IpamError),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io {
                action,
                path,
                source,
            } => write!(f, "failed to {action} {}: {source}", path.display()),
            Self::Corrupt { path, reason } => {
                write!(f, "state file {} is corrupt: {reason}", path.display())
            }
            Self::Ipam(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Corrupt { .. } => None,
            Self::Ipam(error) => Some(error),
        }
    }
}

impl From<StoreError> for CniError {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::Io { .. } => Self::new(ErrorCode::IoFailure, error.to_string()),
            StoreError::Corrupt { .. } => Self::new(ErrorCode::IpamStateCorrupt, error.to_string()),
            StoreError::Ipam(error) => error.into(),
        }
    }
}

/// One persisted lease.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LeaseRecord {
    container_id: String,
    ifname: String,
    address: Ipv4Addr,
}

/// The whole state file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct State {
    version: u32,
    pool: Ipv4Cidr,
    leases: Vec<LeaseRecord>,
}

/// An exclusive handle on the lease file for one pool.
///
/// The lock is held for as long as the `Store` lives and is released when it
/// drops, including on an early return or a panic. So the lock can't outlive
/// the work it protects, and holding a `Store` is proof that this process has
/// exclusive access.
#[derive(Debug)]
pub struct Store {
    path: PathBuf,
    /// Held only for its lock. The file's contents are read and written
    /// separately, because saving replaces the file by rename.
    _lock: File,
}

impl Store {
    /// Opens the state file at `path`, creating it and its parent directory
    /// if needed, and takes the exclusive lock.
    ///
    /// This blocks until any other process holding the lock releases it.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Io`] if the directory or lock file can't be
    /// created or locked.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| StoreError::Io {
                action: "create directory",
                path: parent.to_path_buf(),
                source,
            })?;
        }
        // The lock lives on a file beside the state, not on the state file
        // itself: `save` replaces that file by rename, which would detach a
        // lock held on the old inode.
        let lock_path = lock_path(&path);
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)
            .map_err(|source| StoreError::Io {
                action: "open lock file",
                path: lock_path.clone(),
                source,
            })?;
        lock.lock().map_err(|source| StoreError::Io {
            action: "lock",
            path: lock_path,
            source,
        })?;
        Ok(Self { path, _lock: lock })
    }

    /// The state file's path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Loads the allocator for `pool`.
    ///
    /// A missing file yields an empty allocator, which is the normal state on
    /// a node's first ADD. State saved for a different pool is discarded: the
    /// node's CIDR has been reassigned, so every old lease refers to addresses
    /// the node no longer owns.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Corrupt`] if the file isn't valid state or holds
    /// an unknown format version, [`StoreError::Io`] if it can't be read, and
    /// [`StoreError::Ipam`] if `pool` is unusable or the leases don't fit it.
    pub fn load(&self, pool: Ipv4Cidr) -> Result<Allocator, StoreError> {
        let raw = match std::fs::read(&self.path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == ErrorKind::NotFound => {
                return Allocator::new(pool).map_err(StoreError::Ipam);
            }
            Err(source) => {
                return Err(StoreError::Io {
                    action: "read",
                    path: self.path.clone(),
                    source,
                });
            }
        };
        let state: State = serde_json::from_slice(&raw).map_err(|e| StoreError::Corrupt {
            path: self.path.clone(),
            reason: e.to_string(),
        })?;
        if state.version != FORMAT_VERSION {
            return Err(StoreError::Corrupt {
                path: self.path.clone(),
                reason: format!(
                    "unknown format version {} (this build writes {FORMAT_VERSION})",
                    state.version
                ),
            });
        }
        if state.pool != pool {
            return Allocator::new(pool).map_err(StoreError::Ipam);
        }
        let leases = state.leases.into_iter().map(|lease| {
            (
                LeaseKey::new(lease.container_id, lease.ifname),
                lease.address,
            )
        });
        Allocator::restore(pool, leases).map_err(StoreError::Ipam)
    }

    /// Writes `allocator`'s leases, replacing any previous state.
    ///
    /// The write is atomic: readers see either the old file or the new one,
    /// never a partial write. ADD must call this before returning, so a lease
    /// is durable before the address is handed to a pod (PLAN §5.3).
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Io`] if the state can't be written, flushed, or
    /// renamed into place.
    pub fn save(&self, allocator: &Allocator) -> Result<(), StoreError> {
        let mut leases: Vec<LeaseRecord> = allocator
            .leases()
            .map(|(key, address)| LeaseRecord {
                container_id: key.container_id.clone(),
                ifname: key.ifname.clone(),
                address,
            })
            .collect();
        // A HashMap iterates in an arbitrary order. Sorting keeps the file
        // stable across saves, so a diff shows real changes.
        leases.sort_by_key(|lease| lease.address);
        let state = State {
            version: FORMAT_VERSION,
            pool: allocator.pool(),
            leases,
        };
        let encoded = serde_json::to_vec(&state).map_err(|e| StoreError::Io {
            action: "encode state for",
            path: self.path.clone(),
            source: std::io::Error::other(e),
        })?;
        self.write_atomically(&encoded)
    }

    fn write_atomically(&self, contents: &[u8]) -> Result<(), StoreError> {
        // The temporary file must share a directory with the target: `rename`
        // is only atomic within one filesystem.
        let temp_path = self
            .path
            .with_extension(format!("tmp.{}", std::process::id()));
        let io = |action: &'static str, path: &Path| {
            let path = path.to_path_buf();
            move |source| StoreError::Io {
                action,
                path,
                source,
            }
        };

        let mut temp = File::create(&temp_path).map_err(io("create", &temp_path))?;
        let written = temp
            .write_all(contents)
            // `sync_all` is what makes the data durable. Without it the
            // rename could land while the contents are still only in the page
            // cache, so a power loss would leave an empty file.
            .and_then(|()| temp.sync_all());
        if let Err(source) = written {
            drop(temp);
            let _ = std::fs::remove_file(&temp_path);
            return Err(io("write", &temp_path)(source));
        }
        drop(temp);

        if let Err(source) = std::fs::rename(&temp_path, &self.path) {
            let _ = std::fs::remove_file(&temp_path);
            return Err(io("rename into", &self.path)(source));
        }

        // Flushing the directory persists the rename itself. Without this the
        // file's contents are durable but the name pointing at them may not be.
        if let Some(parent) = self.path.parent()
            && let Ok(dir) = File::open(parent)
        {
            let _ = dir.sync_all();
        }
        Ok(())
    }
}

/// The lock file's path: the state path plus a `.lock` suffix.
fn lock_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".lock");
    PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory under the system temp dir, removed when the test ends.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            // Counter plus PID keeps parallel tests from colliding without
            // pulling in a temp-file dependency.
            use std::sync::atomic::{AtomicU32, Ordering};
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("narrows-ipam-test-{}-{unique}", std::process::id()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn state_path(&self) -> PathBuf {
            self.0.join("leases.json")
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn pool() -> Ipv4Cidr {
        "10.244.1.0/24".parse().unwrap()
    }

    fn key(container_id: &str) -> LeaseKey {
        LeaseKey::new(container_id, "eth0")
    }

    mod load {
        use super::*;

        #[test]
        fn returns_empty_allocator_when_file_missing() {
            let dir = TempDir::new();
            let store = Store::open(dir.state_path()).unwrap();

            assert!(store.load(pool()).unwrap().is_empty());
        }

        #[test]
        fn restores_a_saved_lease() {
            let dir = TempDir::new();
            let store = Store::open(dir.state_path()).unwrap();
            let mut ipam = store.load(pool()).unwrap();
            let address = ipam.allocate(key("a")).unwrap();
            store.save(&ipam).unwrap();

            let reloaded = store.load(pool()).unwrap();

            assert_eq!(reloaded.lease(&key("a")), Some(address));
        }

        #[test]
        fn does_not_reissue_a_persisted_address() {
            let dir = TempDir::new();
            let store = Store::open(dir.state_path()).unwrap();
            let mut first = store.load(pool()).unwrap();
            let taken = first.allocate(key("a")).unwrap();
            store.save(&first).unwrap();

            let mut second = store.load(pool()).unwrap();

            assert_ne!(second.allocate(key("b")).unwrap(), taken);
        }

        #[test]
        fn discards_state_saved_for_a_different_pool() {
            let dir = TempDir::new();
            let store = Store::open(dir.state_path()).unwrap();
            let mut ipam = store.load(pool()).unwrap();
            ipam.allocate(key("a")).unwrap();
            store.save(&ipam).unwrap();

            let reloaded = store.load("10.244.9.0/24".parse().unwrap()).unwrap();

            assert!(reloaded.is_empty());
        }

        #[test]
        fn returns_corrupt_for_malformed_json() {
            let dir = TempDir::new();
            std::fs::write(dir.state_path(), b"{not json").unwrap();
            let store = Store::open(dir.state_path()).unwrap();

            assert!(matches!(
                store.load(pool()),
                Err(StoreError::Corrupt { .. })
            ));
        }

        #[test]
        fn returns_corrupt_for_unknown_format_version() {
            let dir = TempDir::new();
            std::fs::write(
                dir.state_path(),
                br#"{"version":99,"pool":"10.244.1.0/24","leases":[]}"#,
            )
            .unwrap();
            let store = Store::open(dir.state_path()).unwrap();

            assert!(matches!(
                store.load(pool()),
                Err(StoreError::Corrupt { .. })
            ));
        }

        #[test]
        fn returns_ipam_error_when_lease_outside_pool() {
            let dir = TempDir::new();
            std::fs::write(
                dir.state_path(),
                br#"{"version":1,"pool":"10.244.1.0/24",
                     "leases":[{"containerId":"a","ifname":"eth0","address":"10.244.9.5"}]}"#,
            )
            .unwrap();
            let store = Store::open(dir.state_path()).unwrap();

            assert!(matches!(store.load(pool()), Err(StoreError::Ipam(_))));
        }
    }

    mod save {
        use super::*;

        #[test]
        fn creates_the_state_file() {
            let dir = TempDir::new();
            let store = Store::open(dir.state_path()).unwrap();
            let ipam = store.load(pool()).unwrap();

            store.save(&ipam).unwrap();

            assert!(dir.state_path().exists());
        }

        #[test]
        fn creates_missing_parent_directories() {
            let dir = TempDir::new();
            let nested = dir.0.join("var/lib/narrows/leases.json");

            assert!(Store::open(&nested).is_ok());
        }

        #[test]
        fn leaves_no_temporary_files_behind() {
            let dir = TempDir::new();
            let store = Store::open(dir.state_path()).unwrap();
            let ipam = store.load(pool()).unwrap();
            store.save(&ipam).unwrap();

            let strays: Vec<_> = std::fs::read_dir(&dir.0)
                .unwrap()
                .filter_map(Result::ok)
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|name| name.contains("tmp"))
                .collect();

            assert!(strays.is_empty(), "temporary files left: {strays:?}");
        }

        #[test]
        fn writes_leases_sorted_by_address() {
            let dir = TempDir::new();
            let store = Store::open(dir.state_path()).unwrap();
            let mut ipam = store.load(pool()).unwrap();
            for name in ["a", "b", "c"] {
                ipam.allocate(key(name)).unwrap();
            }
            store.save(&ipam).unwrap();

            let raw = std::fs::read(dir.state_path()).unwrap();
            let state: State = serde_json::from_slice(&raw).unwrap();

            let addresses: Vec<_> = state.leases.iter().map(|l| l.address).collect();
            let mut sorted = addresses.clone();
            sorted.sort_unstable();
            assert_eq!(addresses, sorted);
        }

        #[test]
        fn release_is_persisted() {
            let dir = TempDir::new();
            let store = Store::open(dir.state_path()).unwrap();
            let mut ipam = store.load(pool()).unwrap();
            ipam.allocate(key("a")).unwrap();
            store.save(&ipam).unwrap();
            ipam.release(&key("a"));
            store.save(&ipam).unwrap();

            assert!(store.load(pool()).unwrap().is_empty());
        }
    }

    mod error_conversion {
        use super::*;

        #[test]
        fn corrupt_maps_to_narrows_code_102() {
            let error = CniError::from(StoreError::Corrupt {
                path: PathBuf::from("/var/lib/narrows/leases.json"),
                reason: "truncated".into(),
            });

            assert_eq!(error.code().as_u32(), 102);
        }

        #[test]
        fn io_maps_to_spec_code_5() {
            let error = CniError::from(StoreError::Io {
                action: "read",
                path: PathBuf::from("/var/lib/narrows/leases.json"),
                source: std::io::Error::from(ErrorKind::PermissionDenied),
            });

            assert_eq!(error.code(), ErrorCode::IoFailure);
        }
    }
}
