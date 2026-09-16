//! `RTM_NEWADDR`: assigning a pod's address to its container-side interface.
//!
//! # Address bytes vs. integer bytes
//!
//! [`message`](super::message)'s own fields — `rta_len`, `nlmsg_seq`, and so
//! on — are host-native byte order. An address *attribute's value*, such as
//! `IFA_LOCAL`, is different: it's the address's four raw octets, the same
//! order `10.244.1.7` reads left to right. [`Ipv4Addr::octets`] already
//! returns exactly those bytes, so no conversion is needed here — but it's
//! worth naming the distinction, since `RTA_OIF` two files over is a plain
//! integer attribute in the *other* order, inside the very same kind of
//! message.

use std::net::Ipv4Addr;

use super::message::{MessageBuilder, NLM_F_ACK, NLM_F_CREATE, NLM_F_EXCL, NLM_F_REQUEST};

/// `RTM_NEWADDR` (`include/uapi/linux/rtnetlink.h`): add an address to a
/// link.
pub const RTM_NEWADDR: u16 = 20;

const AF_INET: u8 = 2;

// `IFA_*` (`include/uapi/linux/if_addr.h`).
const IFA_ADDRESS: u16 = 1;
const IFA_LOCAL: u16 = 2;

/// `RT_SCOPE_UNIVERSE` (`include/uapi/linux/rtnetlink.h`): reachable from
/// anywhere, not just this link. What `ip addr add` uses by default.
const RT_SCOPE_UNIVERSE: u8 = 0;

/// `struct ifaddrmsg` (`include/uapi/linux/if_addr.h`), 8 bytes and already
/// 4-byte aligned: `ifa_family`, `ifa_prefixlen`, `ifa_flags`, `ifa_scope`,
/// `ifa_index`.
fn ifaddrmsg(prefix_len: u8, index: u32) -> [u8; 8] {
    let mut bytes = [0u8; 8];
    bytes[0] = AF_INET;
    bytes[1] = prefix_len;
    // ifa_flags (byte 2) is left 0: no special flags requested.
    bytes[3] = RT_SCOPE_UNIVERSE;
    bytes[4..8].copy_from_slice(&index.to_ne_bytes());
    bytes
}

/// Builds an `RTM_NEWADDR` request assigning `address/prefix_len` to the
/// interface at `ifindex`.
///
/// `IFA_ADDRESS` and `IFA_LOCAL` both carry `address`. The kernel only tells
/// them apart on a point-to-point link with a separate peer address —
/// Narrows' routed `/32` model doesn't use one; the gateway lives outside the
/// address entirely, as a route and a neighbour entry, not as a peer
/// (PLAN §6.1).
#[must_use]
pub fn add_address(address: Ipv4Addr, prefix_len: u8, ifindex: u32, seq: u32) -> Vec<u8> {
    let flags = NLM_F_REQUEST | NLM_F_ACK | NLM_F_CREATE | NLM_F_EXCL;
    let mut msg = MessageBuilder::new(RTM_NEWADDR, flags, seq);
    msg.push_bytes(&ifaddrmsg(prefix_len, ifindex));
    msg.push_attr(IFA_LOCAL, &address.octets());
    msg.push_attr(IFA_ADDRESS, &address.octets());
    msg.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::netlink::message::{AttributeIter, parse_header};

    fn find_attr(bytes: &[u8], attr_type: u16) -> &[u8] {
        AttributeIter::new(bytes)
            .find_map(|attr| match attr {
                Ok((t, value)) if t == attr_type => Some(value),
                _ => None,
            })
            .unwrap_or_else(|| panic!("attribute {attr_type} not found in {bytes:?}"))
    }

    mod add_address {
        use super::*;

        #[test]
        fn header_is_rtm_newaddr_with_create_and_exclusive_flags() {
            let msg = add_address(Ipv4Addr::new(10, 244, 1, 7), 32, 4, 0);

            let (header, _) = parse_header(&msg).unwrap();

            assert_eq!(header.msg_type, RTM_NEWADDR);
            assert_eq!(
                header.flags,
                NLM_F_REQUEST | NLM_F_ACK | NLM_F_CREATE | NLM_F_EXCL
            );
        }

        #[test]
        fn ifaddrmsg_carries_prefix_len_and_ifindex() {
            let msg = add_address(Ipv4Addr::new(10, 244, 1, 7), 32, 9, 0);
            let (_, payload) = parse_header(&msg).unwrap();

            let ifaddrmsg = &payload[..8];

            assert_eq!(ifaddrmsg[1], 32, "ifa_prefixlen");
            assert_eq!(
                u32::from_ne_bytes(ifaddrmsg[4..8].try_into().unwrap()),
                9,
                "ifa_index"
            );
        }

        #[test]
        fn local_and_address_attrs_carry_the_pod_address() {
            let address = Ipv4Addr::new(10, 244, 1, 7);
            let msg = add_address(address, 32, 4, 0);
            let (_, payload) = parse_header(&msg).unwrap();

            let local = find_attr(&payload[8..], IFA_LOCAL);
            let addr = find_attr(&payload[8..], IFA_ADDRESS);

            assert_eq!(local, address.octets());
            assert_eq!(addr, address.octets());
        }

        #[test]
        fn message_length_is_a_multiple_of_four() {
            let msg = add_address(Ipv4Addr::new(10, 244, 1, 7), 32, 4, 0);

            assert_eq!(msg.len() % 4, 0);
        }
    }
}
