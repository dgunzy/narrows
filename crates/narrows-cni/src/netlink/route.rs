//! `RTM_NEWROUTE`: the two routes a routed `/32` pod needs (PLAN §6.1) — the
//! pod's own default route to the fixed link-local gateway, and the host's
//! route back to the pod.
//!
//! `RTA_GATEWAY` and `RTA_DST` hold raw address bytes, the same as
//! [`IFA_LOCAL`](super::addr) in the address module. `RTA_OIF` doesn't: an
//! interface index is a plain integer, not an address, so it's encoded the
//! same host-native way as everything in [`message`](super::message) itself.
//! Two attributes that look alike — four bytes, right next to each other in
//! the same message — and one of them is byte-order-sensitive while the
//! other isn't. Nothing about the wire format flags which is which; only
//! knowing what the attribute means does.

use std::net::Ipv4Addr;

use super::message::{MessageBuilder, NLM_F_ACK, NLM_F_CREATE, NLM_F_EXCL, NLM_F_REQUEST};

/// `RTM_NEWROUTE` (`include/uapi/linux/rtnetlink.h`): add a route.
pub const RTM_NEWROUTE: u16 = 24;

const AF_INET: u8 = 2;

// `RTA_*` (`include/uapi/linux/rtnetlink.h`).
const RTA_DST: u16 = 1;
const RTA_OIF: u16 = 4;
const RTA_GATEWAY: u16 = 5;

const RT_TABLE_MAIN: u8 = 254;
/// Reachable only through a directly attached link, no gateway needed —
/// what a plain "route to this device" gets.
const RT_SCOPE_LINK: u8 = 253;
/// Reachable through a gateway, potentially off-link — what a route with an
/// `RTA_GATEWAY` gets.
const RT_SCOPE_UNIVERSE: u8 = 0;
/// `RTPROT_STATIC`: installed by an administrator (or, here, a CNI plugin),
/// not by the kernel itself at boot (`RTPROT_BOOT`) or by a routing daemon.
const RTPROT_STATIC: u8 = 4;
const RTN_UNICAST: u8 = 1;

/// `struct rtmsg` (`include/uapi/linux/rtnetlink.h`), 12 bytes and already
/// 4-byte aligned: `rtm_family`, `rtm_dst_len`, `rtm_src_len`, `rtm_tos`,
/// `rtm_table`, `rtm_protocol`, `rtm_scope`, `rtm_type`, `rtm_flags`.
fn rtmsg(dst_len: u8, scope: u8) -> [u8; 12] {
    let mut bytes = [0u8; 12];
    bytes[0] = AF_INET;
    bytes[1] = dst_len;
    // rtm_src_len and rtm_tos (bytes 2..4) are left 0: no source routing.
    bytes[4] = RT_TABLE_MAIN;
    bytes[5] = RTPROT_STATIC;
    bytes[6] = scope;
    bytes[7] = RTN_UNICAST;
    // rtm_flags (bytes 8..12) is left 0.
    bytes
}

/// Builds an `RTM_NEWROUTE` request for a default route (`0.0.0.0/0`) via
/// `gateway`, out of the interface at `ifindex` — the route every pod gets
/// to Narrows' fixed link-local gateway.
///
/// A default route has no destination to narrow down: unlike
/// [`add_host_route`], this sends no `RTA_DST` at all, the same way
/// `ip route add default via ...` doesn't. `rtm_dst_len` staying `0` is
/// what says "every destination" on its own.
#[must_use]
pub fn add_default_route(gateway: Ipv4Addr, ifindex: u32, seq: u32) -> Vec<u8> {
    let flags = NLM_F_REQUEST | NLM_F_ACK | NLM_F_CREATE | NLM_F_EXCL;
    let mut msg = MessageBuilder::new(RTM_NEWROUTE, flags, seq);
    msg.push_bytes(&rtmsg(0, RT_SCOPE_UNIVERSE));
    msg.push_attr(RTA_GATEWAY, &gateway.octets());
    msg.push_attr(RTA_OIF, &ifindex.to_ne_bytes());
    msg.finish()
}

/// Builds an `RTM_NEWROUTE` request for the host's `/32` route to `pod`,
/// directly out of the interface at `ifindex` — no gateway, since the pod is
/// one hop away, at the other end of the veth pair.
#[must_use]
pub fn add_host_route(pod: Ipv4Addr, ifindex: u32, seq: u32) -> Vec<u8> {
    let flags = NLM_F_REQUEST | NLM_F_ACK | NLM_F_CREATE | NLM_F_EXCL;
    let mut msg = MessageBuilder::new(RTM_NEWROUTE, flags, seq);
    msg.push_bytes(&rtmsg(32, RT_SCOPE_LINK));
    msg.push_attr(RTA_DST, &pod.octets());
    msg.push_attr(RTA_OIF, &ifindex.to_ne_bytes());
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

    /// This exact-byte test pins the wire format down precisely for a route
    /// with no nesting, small enough to hand-verify. Like the ones in
    /// `message.rs`, it only runs where a literal little-endian array is the
    /// right expectation — every target this project ships to (PLAN §2.2).
    #[cfg(target_endian = "little")]
    #[test]
    fn default_route_matches_hand_computed_bytes() {
        let msg = add_default_route(Ipv4Addr::new(169, 254, 1, 1), 5, 3);

        #[rustfmt::skip]
        let expected: &[u8] = &[
            // Header (16B): len=44, type=RTM_NEWROUTE(24),
            // flags=REQUEST|ACK|CREATE|EXCL(0x0605), seq=3, pid=0.
            44, 0, 0, 0,  24, 0,  0x05, 0x06,  3, 0, 0, 0,  0, 0, 0, 0,
            // rtmsg (12B): family=AF_INET(2), dst_len=0, src_len=0, tos=0,
            // table=254, protocol=RTPROT_STATIC(4), scope=UNIVERSE(0),
            // type=RTN_UNICAST(1), flags=0.
            2, 0, 0, 0,  254, 4, 0, 1,  0, 0, 0, 0,
            // RTA_GATEWAY (8B): rta_len=8, rta_type=5, value=169.254.1.1.
            8, 0,  5, 0,  169, 254, 1, 1,
            // RTA_OIF (8B): rta_len=8, rta_type=4, value=5 (native order).
            8, 0,  4, 0,  5, 0, 0, 0,
        ];
        assert_eq!(msg, expected);
    }

    mod add_default_route {
        use super::*;

        #[test]
        fn header_is_rtm_newroute() {
            let msg = add_default_route(Ipv4Addr::new(169, 254, 1, 1), 5, 0);

            let (header, _) = parse_header(&msg).unwrap();

            assert_eq!(header.msg_type, RTM_NEWROUTE);
        }

        #[test]
        fn dst_len_is_zero() {
            let msg = add_default_route(Ipv4Addr::new(169, 254, 1, 1), 5, 0);
            let (_, payload) = parse_header(&msg).unwrap();

            assert_eq!(payload[1], 0, "rtm_dst_len");
        }

        #[test]
        fn sends_no_rta_dst() {
            let msg = add_default_route(Ipv4Addr::new(169, 254, 1, 1), 5, 0);
            let (_, payload) = parse_header(&msg).unwrap();

            let has_dst = AttributeIter::new(&payload[12..]).any(|a| matches!(a, Ok((RTA_DST, _))));

            assert!(!has_dst, "default route should not send RTA_DST");
        }

        #[test]
        fn gateway_attr_carries_the_gateway_address() {
            let gateway = Ipv4Addr::new(169, 254, 1, 1);
            let msg = add_default_route(gateway, 5, 0);
            let (_, payload) = parse_header(&msg).unwrap();

            let value = find_attr(&payload[12..], RTA_GATEWAY);

            assert_eq!(value, gateway.octets());
        }

        #[test]
        fn oif_attr_carries_the_ifindex_in_native_order() {
            let msg = add_default_route(Ipv4Addr::new(169, 254, 1, 1), 7, 0);
            let (_, payload) = parse_header(&msg).unwrap();

            let value = find_attr(&payload[12..], RTA_OIF);

            assert_eq!(value, 7u32.to_ne_bytes());
        }
    }

    mod add_host_route {
        use super::*;

        #[test]
        fn dst_len_is_32() {
            let msg = add_host_route(Ipv4Addr::new(10, 244, 1, 7), 8, 0);
            let (_, payload) = parse_header(&msg).unwrap();

            assert_eq!(payload[1], 32, "rtm_dst_len");
        }

        #[test]
        fn sends_no_gateway() {
            let msg = add_host_route(Ipv4Addr::new(10, 244, 1, 7), 8, 0);
            let (_, payload) = parse_header(&msg).unwrap();

            let has_gateway =
                AttributeIter::new(&payload[12..]).any(|a| matches!(a, Ok((RTA_GATEWAY, _))));

            assert!(
                !has_gateway,
                "a directly attached route should not send RTA_GATEWAY"
            );
        }

        #[test]
        fn dst_attr_carries_the_pod_address() {
            let pod = Ipv4Addr::new(10, 244, 1, 7);
            let msg = add_host_route(pod, 8, 0);
            let (_, payload) = parse_header(&msg).unwrap();

            let value = find_attr(&payload[12..], RTA_DST);

            assert_eq!(value, pod.octets());
        }
    }
}
