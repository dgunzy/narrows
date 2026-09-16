//! The in-memory address allocator: a bitmap over the pool, searched next-fit.

use std::collections::HashMap;
use std::fmt;
use std::net::Ipv4Addr;

use super::cidr::Ipv4Cidr;
use crate::error::{CniError, ErrorCode};

/// The shortest pool prefix accepted. A `/16` holds 65,536 addresses, which
/// keeps the bitmap at 1 KiB and a full scan cheap. Node pod CIDRs are
/// usually `/24`.
pub const MIN_POOL_PREFIX_LEN: u8 = 16;

/// The longest pool prefix accepted. A `/30` has four addresses. After
/// reserving network and broadcast, two are left for pods.
pub const MAX_POOL_PREFIX_LEN: u8 = 30;

/// Identifies one attachment: a container's interface.
///
/// The spec identifies an attachment by `(CNI_CONTAINERID, CNI_IFNAME)`, not
/// by container alone, because one container can have several interfaces.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LeaseKey {
    /// `CNI_CONTAINERID`.
    pub container_id: String,
    /// `CNI_IFNAME`.
    pub ifname: String,
}

impl LeaseKey {
    /// Creates a key from a container ID and an interface name.
    pub fn new(container_id: impl Into<String>, ifname: impl Into<String>) -> Self {
        Self {
            container_id: container_id.into(),
            ifname: ifname.into(),
        }
    }
}

/// Why an allocator operation failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IpamError {
    /// The pool's prefix is outside
    /// [`MIN_POOL_PREFIX_LEN`]..=[`MAX_POOL_PREFIX_LEN`].
    PoolPrefixLenOutOfRange(Ipv4Cidr),
    /// Every usable address in the pool is leased.
    Exhausted(Ipv4Cidr),
    /// A restored lease names an address the pool doesn't contain.
    AddressOutsidePool {
        /// The pool being restored into.
        pool: Ipv4Cidr,
        /// The address that doesn't belong to it.
        address: Ipv4Addr,
    },
    /// Two restored leases claim the same address.
    DuplicateAddress {
        /// The address claimed twice.
        address: Ipv4Addr,
    },
}

impl fmt::Display for IpamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PoolPrefixLenOutOfRange(pool) => write!(
                f,
                "pool {pool} must have a prefix length from /{MIN_POOL_PREFIX_LEN} to /{MAX_POOL_PREFIX_LEN}"
            ),
            Self::Exhausted(pool) => write!(f, "pool {pool} has no free addresses"),
            Self::AddressOutsidePool { pool, address } => {
                write!(f, "leased address {address} is outside pool {pool}")
            }
            Self::DuplicateAddress { address } => {
                write!(f, "address {address} is leased twice")
            }
        }
    }
}

impl std::error::Error for IpamError {}

impl From<IpamError> for CniError {
    fn from(error: IpamError) -> Self {
        let code = match error {
            IpamError::PoolPrefixLenOutOfRange(_) => ErrorCode::InvalidNetworkConfig,
            IpamError::Exhausted(_) => ErrorCode::IpamExhausted,
            IpamError::AddressOutsidePool { .. } | IpamError::DuplicateAddress { .. } => {
                ErrorCode::IpamStateCorrupt
            }
        };
        Self::new(code, error.to_string())
    }
}

/// Hands out addresses from one pool, one per [`LeaseKey`].
///
/// A bitmap tracks free/used per address; the lease map tracks who holds
/// which one. Allocation is next-fit, not first-fit: reusing a just-released
/// address immediately risks handing it to a new pod while something else
/// (a conntrack entry, a cached neighbour entry) still expects the old one
/// (PLAN §5.3, §14.2). Network and broadcast are always reserved, matching
/// Kubernetes' `ipallocator` and CNI's `host-local` — pods here are routed
/// `/32`s, so neither address is technically special, but tools and people
/// still treat them that way.
#[derive(Debug, Clone)]
pub struct Allocator {
    pool: Ipv4Cidr,
    /// Addresses in the pool, as a `u32` for cheap offset arithmetic.
    size: u32,
    /// Bit `n` is set when offset `n` is leased or reserved.
    used: Vec<u64>,
    leases: HashMap<LeaseKey, u32>,
    /// The offset most recently allocated. The next search starts after it.
    cursor: u32,
}

impl Allocator {
    /// Creates an empty allocator for `pool`, with its network and broadcast
    /// addresses reserved.
    ///
    /// # Errors
    ///
    /// Returns [`IpamError::PoolPrefixLenOutOfRange`] unless the pool's
    /// prefix length is between [`MIN_POOL_PREFIX_LEN`] and
    /// [`MAX_POOL_PREFIX_LEN`].
    pub fn new(pool: Ipv4Cidr) -> Result<Self, IpamError> {
        if !(MIN_POOL_PREFIX_LEN..=MAX_POOL_PREFIX_LEN).contains(&pool.prefix_len()) {
            return Err(IpamError::PoolPrefixLenOutOfRange(pool));
        }
        let size =
            u32::try_from(pool.size()).map_err(|_| IpamError::PoolPrefixLenOutOfRange(pool))?;
        let mut allocator = Self {
            pool,
            size,
            used: vec![0; size.div_ceil(64) as usize],
            leases: HashMap::new(),
            cursor: 0,
        };
        allocator.set(0, true);
        allocator.set(size - 1, true);
        Ok(allocator)
    }

    /// Rebuilds an allocator from leases loaded off disk.
    ///
    /// The cursor is left at the highest restored offset, so allocation
    /// resumes past the most recent lease rather than reusing a low address
    /// the moment the process restarts.
    ///
    /// # Errors
    ///
    /// Returns [`IpamError::PoolPrefixLenOutOfRange`] if `pool` is unusable,
    /// [`IpamError::AddressOutsidePool`] if a lease doesn't belong to `pool`,
    /// and [`IpamError::DuplicateAddress`] if two leases claim one address.
    pub fn restore(
        pool: Ipv4Cidr,
        leases: impl IntoIterator<Item = (LeaseKey, Ipv4Addr)>,
    ) -> Result<Self, IpamError> {
        let mut allocator = Self::new(pool)?;
        for (key, address) in leases {
            let offset = pool
                .offset_of(address)
                .and_then(|offset| u32::try_from(offset).ok())
                .ok_or(IpamError::AddressOutsidePool { pool, address })?;
            // The reserved network and broadcast offsets are already set, so
            // this also rejects state claiming one of them.
            if allocator.get(offset) {
                return Err(IpamError::DuplicateAddress { address });
            }
            allocator.set(offset, true);
            allocator.cursor = allocator.cursor.max(offset);
            allocator.leases.insert(key, offset);
        }
        Ok(allocator)
    }

    /// Every lease held, in arbitrary order.
    pub fn leases(&self) -> impl Iterator<Item = (&LeaseKey, Ipv4Addr)> {
        self.leases
            .iter()
            .map(|(key, &offset)| (key, self.addr_at(offset)))
    }

    /// The pool this allocator hands out from.
    #[must_use]
    pub const fn pool(&self) -> Ipv4Cidr {
        self.pool
    }

    /// Returns the address leased to `key`, allocating one if it has none.
    ///
    /// Calling this again with the same key returns the same address. A
    /// runtime may retry an ADD that timed out, and the retry must not leak
    /// a second address.
    ///
    /// # Errors
    ///
    /// Returns [`IpamError::Exhausted`] if `key` has no lease and every
    /// usable address is taken.
    pub fn allocate(&mut self, key: LeaseKey) -> Result<Ipv4Addr, IpamError> {
        if let Some(&offset) = self.leases.get(&key) {
            return Ok(self.addr_at(offset));
        }
        let offset = (1..self.size)
            .map(|step| (self.cursor + step) % self.size)
            .find(|&offset| !self.get(offset))
            .ok_or(IpamError::Exhausted(self.pool))?;
        self.set(offset, true);
        self.cursor = offset;
        self.leases.insert(key, offset);
        Ok(self.addr_at(offset))
    }

    /// Releases the lease held by `key`, returning the address it held.
    ///
    /// An unknown key returns `None` and changes nothing. That keeps DEL
    /// idempotent: libcni may send DEL for an attachment Narrows has already
    /// forgotten (PLAN §2.1).
    pub fn release(&mut self, key: &LeaseKey) -> Option<Ipv4Addr> {
        let offset = self.leases.remove(key)?;
        self.set(offset, false);
        Some(self.addr_at(offset))
    }

    /// The address leased to `key`, if any.
    #[must_use]
    pub fn lease(&self, key: &LeaseKey) -> Option<Ipv4Addr> {
        self.leases.get(key).map(|&offset| self.addr_at(offset))
    }

    /// How many leases are held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.leases.len()
    }

    /// Whether no leases are held.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.leases.is_empty()
    }

    /// The address at `offset`. The pool's host bits are zero and
    /// `offset < size`, so OR-ing the offset into the network address can't
    /// carry into the network bits.
    fn addr_at(&self, offset: u32) -> Ipv4Addr {
        Ipv4Addr::from(u32::from(self.pool.network()) | offset)
    }

    fn get(&self, offset: u32) -> bool {
        self.used[(offset / 64) as usize] & (1 << (offset % 64)) != 0
    }

    fn set(&mut self, offset: u32, value: bool) {
        let word = &mut self.used[(offset / 64) as usize];
        if value {
            *word |= 1 << (offset % 64);
        } else {
            *word &= !(1 << (offset % 64));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allocator(pool: &str) -> Allocator {
        Allocator::new(pool.parse().unwrap()).unwrap()
    }

    fn key(container_id: &str) -> LeaseKey {
        LeaseKey::new(container_id, "eth0")
    }

    mod new {
        use super::*;

        #[test]
        fn accepts_slash_24() {
            assert!(Allocator::new("10.244.1.0/24".parse().unwrap()).is_ok());
        }

        #[test]
        fn rejects_slash_31() {
            let pool = "10.244.1.0/31".parse().unwrap();

            assert_eq!(
                Allocator::new(pool).unwrap_err(),
                IpamError::PoolPrefixLenOutOfRange(pool)
            );
        }

        #[test]
        fn rejects_slash_15() {
            let pool = "10.244.0.0/15".parse().unwrap();

            assert_eq!(
                Allocator::new(pool).unwrap_err(),
                IpamError::PoolPrefixLenOutOfRange(pool)
            );
        }
    }

    mod allocate {
        use super::*;

        #[test]
        fn first_lease_skips_network_address() {
            let mut ipam = allocator("10.244.1.0/24");

            assert_eq!(ipam.allocate(key("a")), Ok(Ipv4Addr::new(10, 244, 1, 1)));
        }

        #[test]
        fn returns_same_address_for_same_key() {
            let mut ipam = allocator("10.244.1.0/24");
            let first = ipam.allocate(key("a")).unwrap();

            assert_eq!(ipam.allocate(key("a")), Ok(first));
        }

        #[test]
        fn repeat_allocation_does_not_consume_an_address() {
            let mut ipam = allocator("10.244.1.0/24");
            ipam.allocate(key("a")).unwrap();
            ipam.allocate(key("a")).unwrap();

            assert_eq!(ipam.allocate(key("b")), Ok(Ipv4Addr::new(10, 244, 1, 2)));
        }

        #[test]
        fn gives_second_interface_of_same_container_its_own_address() {
            let mut ipam = allocator("10.244.1.0/24");
            ipam.allocate(LeaseKey::new("a", "eth0")).unwrap();

            assert_eq!(
                ipam.allocate(LeaseKey::new("a", "eth1")),
                Ok(Ipv4Addr::new(10, 244, 1, 2))
            );
        }

        #[test]
        fn does_not_reuse_released_address_immediately() {
            let mut ipam = allocator("10.244.1.0/24");
            ipam.allocate(key("a")).unwrap();
            ipam.allocate(key("b")).unwrap();
            ipam.release(&key("a"));

            assert_eq!(ipam.allocate(key("c")), Ok(Ipv4Addr::new(10, 244, 1, 3)));
        }

        #[test]
        fn wraps_around_to_released_address_and_skips_broadcast() {
            // A /30 has .0 (network), .1, .2, and .3 (broadcast).
            let mut ipam = allocator("10.244.1.0/30");
            ipam.allocate(key("a")).unwrap();
            ipam.allocate(key("b")).unwrap();
            ipam.release(&key("a"));

            assert_eq!(ipam.allocate(key("c")), Ok(Ipv4Addr::new(10, 244, 1, 1)));
        }

        #[test]
        fn returns_exhausted_when_pool_full() {
            let mut ipam = allocator("10.244.1.0/30");
            ipam.allocate(key("a")).unwrap();
            ipam.allocate(key("b")).unwrap();

            assert_eq!(
                ipam.allocate(key("c")),
                Err(IpamError::Exhausted(ipam.pool()))
            );
        }

        #[test]
        fn fills_every_usable_address_in_slash_24() {
            let mut ipam = allocator("10.244.1.0/24");
            for i in 0..254 {
                ipam.allocate(key(&format!("c{i}"))).unwrap();
            }

            assert_eq!(ipam.len(), 254);
        }
    }

    mod release {
        use super::*;

        #[test]
        fn returns_released_address() {
            let mut ipam = allocator("10.244.1.0/24");
            let addr = ipam.allocate(key("a")).unwrap();

            assert_eq!(ipam.release(&key("a")), Some(addr));
        }

        #[test]
        fn returns_none_for_unknown_key() {
            let mut ipam = allocator("10.244.1.0/24");

            assert_eq!(ipam.release(&key("never-added")), None);
        }

        #[test]
        fn forgets_the_lease() {
            let mut ipam = allocator("10.244.1.0/24");
            ipam.allocate(key("a")).unwrap();
            ipam.release(&key("a"));

            assert_eq!(ipam.lease(&key("a")), None);
        }
    }

    mod error_conversion {
        use super::*;

        #[test]
        fn exhausted_maps_to_narrows_code_101() {
            let error = CniError::from(IpamError::Exhausted("10.244.1.0/30".parse().unwrap()));

            assert_eq!(error.code().as_u32(), 101);
        }
    }
}
