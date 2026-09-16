//! IPv4 CIDR blocks, such as a node's pod range `10.244.1.0/24`.
//!
//! A CIDR block is a base address plus a prefix length. The first
//! `prefix_len` bits are fixed (the network part), and the remaining
//! `32 - prefix_len` bits are free (the host part). So a `/24` holds
//! `2^8 = 256` addresses, and a `/32` holds exactly one.
//!
//! Under the dotted-quad notation, an IPv4 address is a `u32`, and every
//! operation here is bit arithmetic on that `u32`.

use std::fmt;
use std::net::Ipv4Addr;
use std::str::FromStr;

/// An IPv4 CIDR block whose host bits are all zero.
///
/// The fields are private, so the only ways to build one are [`Ipv4Cidr::new`]
/// and parsing. Both enforce the invariant, so every other method can rely
/// on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Ipv4Cidr {
    network: Ipv4Addr,
    prefix_len: u8,
}

/// Why a CIDR couldn't be built or parsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CidrError {
    /// The string has no `/` separating address and prefix length.
    MissingSlash,
    /// The part before `/` isn't a dotted-quad IPv4 address.
    InvalidAddress,
    /// The prefix length isn't a plain decimal number from 0 to 32.
    InvalidPrefixLen,
    /// The address has bits set outside the prefix, as in `10.244.1.7/24`.
    HostBitsSet,
}

impl fmt::Display for CidrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::MissingSlash => "expected ADDRESS/PREFIX_LEN",
            Self::InvalidAddress => "invalid IPv4 address",
            Self::InvalidPrefixLen => "prefix length must be a number from 0 to 32",
            Self::HostBitsSet => "address has host bits set for this prefix length",
        })
    }
}

impl std::error::Error for CidrError {}

impl Ipv4Cidr {
    /// `0.0.0.0/0`: the block containing every IPv4 address, used for a
    /// default route.
    pub const UNSPECIFIED: Self = Self {
        network: Ipv4Addr::UNSPECIFIED,
        prefix_len: 0,
    };

    /// Builds a CIDR from a network address and a prefix length.
    ///
    /// # Errors
    ///
    /// Returns [`CidrError::InvalidPrefixLen`] if `prefix_len > 32`, and
    /// [`CidrError::HostBitsSet`] if `network` has any bit set past the
    /// prefix.
    pub fn new(network: Ipv4Addr, prefix_len: u8) -> Result<Self, CidrError> {
        if prefix_len > 32 {
            return Err(CidrError::InvalidPrefixLen);
        }
        if u32::from(network) & !mask(prefix_len) != 0 {
            return Err(CidrError::HostBitsSet);
        }
        Ok(Self {
            network,
            prefix_len,
        })
    }

    /// The first address in the block.
    #[must_use]
    pub const fn network(&self) -> Ipv4Addr {
        self.network
    }

    /// The number of fixed leading bits.
    #[must_use]
    pub const fn prefix_len(&self) -> u8 {
        self.prefix_len
    }

    /// How many addresses the block holds, including network and broadcast.
    ///
    /// This returns `u64` because a `/0` holds 2^32 addresses, one more than
    /// `u32::MAX`.
    #[must_use]
    pub fn size(&self) -> u64 {
        1u64 << (32 - self.prefix_len)
    }

    /// Whether `addr` falls inside the block.
    #[must_use]
    pub fn contains(&self, addr: Ipv4Addr) -> bool {
        u32::from(addr) & mask(self.prefix_len) == u32::from(self.network)
    }

    /// How far `addr` is from the network address, or `None` if `addr` is
    /// outside the block.
    ///
    /// For `10.244.1.0/24`, `10.244.1.0` is offset 0 and `10.244.1.255` is
    /// offset 255.
    #[must_use]
    pub fn offset_of(&self, addr: Ipv4Addr) -> Option<u64> {
        self.contains(addr)
            .then(|| u64::from(u32::from(addr) - u32::from(self.network)))
    }
}

/// The netmask for `prefix_len` as a `u32`: `prefix_len` one-bits, then zeros.
fn mask(prefix_len: u8) -> u32 {
    u32::MAX
        .checked_shl(32 - u32::from(prefix_len))
        .unwrap_or(0)
}

impl FromStr for Ipv4Cidr {
    type Err = CidrError;

    /// Parses `ADDRESS/PREFIX_LEN`, for example `10.244.1.0/24`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (addr, prefix_len) = s.split_once('/').ok_or(CidrError::MissingSlash)?;
        let network = addr.parse().map_err(|_| CidrError::InvalidAddress)?;
        if prefix_len.is_empty() || !prefix_len.bytes().all(|b| b.is_ascii_digit()) {
            return Err(CidrError::InvalidPrefixLen);
        }
        let prefix_len = prefix_len
            .parse()
            .map_err(|_| CidrError::InvalidPrefixLen)?;
        Self::new(network, prefix_len)
    }
}

impl fmt::Display for Ipv4Cidr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.network, self.prefix_len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cidr(s: &str) -> Ipv4Cidr {
        s.parse().unwrap()
    }

    mod from_str {
        use super::*;

        #[test]
        fn parses_pod_cidr() {
            assert_eq!(
                "10.244.1.0/24".parse(),
                Ipv4Cidr::new(Ipv4Addr::new(10, 244, 1, 0), 24)
            );
        }

        #[test]
        fn parses_slash_zero() {
            assert_eq!(cidr("0.0.0.0/0").prefix_len(), 0);
        }

        #[test]
        fn parses_slash_32() {
            assert_eq!(cidr("10.0.0.7/32").network(), Ipv4Addr::new(10, 0, 0, 7));
        }

        #[test]
        fn rejects_missing_slash() {
            assert_eq!(
                "10.244.1.0".parse::<Ipv4Cidr>(),
                Err(CidrError::MissingSlash)
            );
        }

        #[test]
        fn rejects_invalid_address() {
            assert_eq!(
                "10.244.1/24".parse::<Ipv4Cidr>(),
                Err(CidrError::InvalidAddress)
            );
        }

        #[test]
        fn rejects_prefix_over_32() {
            assert_eq!(
                "10.0.0.0/33".parse::<Ipv4Cidr>(),
                Err(CidrError::InvalidPrefixLen)
            );
        }

        #[test]
        fn rejects_empty_prefix() {
            assert_eq!(
                "10.0.0.0/".parse::<Ipv4Cidr>(),
                Err(CidrError::InvalidPrefixLen)
            );
        }

        #[test]
        fn rejects_plus_sign_in_prefix() {
            assert_eq!(
                "10.0.0.0/+24".parse::<Ipv4Cidr>(),
                Err(CidrError::InvalidPrefixLen)
            );
        }

        #[test]
        fn rejects_host_bits_set() {
            assert_eq!(
                "10.244.1.7/24".parse::<Ipv4Cidr>(),
                Err(CidrError::HostBitsSet)
            );
        }
    }

    mod size {
        use super::*;

        #[test]
        fn slash_24_holds_256() {
            assert_eq!(cidr("10.244.1.0/24").size(), 256);
        }

        #[test]
        fn slash_32_holds_one() {
            assert_eq!(cidr("10.0.0.7/32").size(), 1);
        }

        #[test]
        fn slash_zero_holds_two_to_the_32() {
            assert_eq!(cidr("0.0.0.0/0").size(), 1 << 32);
        }
    }

    mod offset_of {
        use super::*;

        #[test]
        fn network_address_is_offset_zero() {
            let pool = cidr("10.244.1.0/24");

            assert_eq!(pool.offset_of(Ipv4Addr::new(10, 244, 1, 0)), Some(0));
        }

        #[test]
        fn last_address_is_offset_size_minus_one() {
            let pool = cidr("10.244.1.0/24");

            assert_eq!(pool.offset_of(Ipv4Addr::new(10, 244, 1, 255)), Some(255));
        }

        #[test]
        fn returns_none_for_next_block() {
            let pool = cidr("10.244.1.0/24");

            assert_eq!(pool.offset_of(Ipv4Addr::new(10, 244, 2, 0)), None);
        }
    }

    mod unspecified {
        use super::*;

        #[test]
        fn is_0_0_0_0_slash_0() {
            assert_eq!(Ipv4Cidr::UNSPECIFIED, "0.0.0.0/0".parse().unwrap());
        }

        #[test]
        fn contains_every_address() {
            assert!(Ipv4Cidr::UNSPECIFIED.contains(Ipv4Addr::BROADCAST));
        }
    }

    mod display {
        use super::*;

        #[test]
        fn round_trips_through_from_str() {
            assert_eq!(cidr("10.244.1.0/24").to_string(), "10.244.1.0/24");
        }
    }
}
