//! `RTM_NEWLINK`: creating the veth pair behind a pod's interface.
//!
//! A veth device is a virtual Ethernet cable: creating one gives you two
//! linked interfaces, and whatever goes in one comes out the other. Narrows
//! puts one end in the pod's network namespace as `eth0` and leaves the other
//! on the host, which is how the routed `/32` model connects a pod to the
//! node without a bridge (PLAN §6.1).
//!
//! `ip link add narrows0 type veth peer name tmp-abc123` is the same request
//! this module builds by hand. Creating the pair is one message; moving the
//! peer into the pod's namespace and renaming it to `CNI_IFNAME` are separate
//! `RTM_NEWLINK` requests, not built here yet.

use super::message::{MessageBuilder, NLM_F_ACK, NLM_F_CREATE, NLM_F_EXCL, NLM_F_REQUEST};

/// `RTM_NEWLINK` (`include/uapi/linux/rtnetlink.h`): create or update a link.
pub const RTM_NEWLINK: u16 = 16;
/// `RTM_DELLINK` (`include/uapi/linux/rtnetlink.h`): delete a link. Deleting
/// either end of a veth pair deletes both.
pub const RTM_DELLINK: u16 = 17;

/// `AF_UNSPEC`: left for the kernel to fill in, the way `ip link add` does.
const AF_UNSPEC: u8 = 0;

// `IFLA_*` (`include/uapi/linux/if_link.h`).
const IFLA_IFNAME: u16 = 3;
const IFLA_LINKINFO: u16 = 18;

// The nested attributes inside `IFLA_LINKINFO`.
const IFLA_INFO_KIND: u16 = 1;
const IFLA_INFO_DATA: u16 = 2;

/// `VETH_INFO_PEER` (`include/uapi/linux/veth.h`): the nested attribute
/// inside `IFLA_INFO_DATA` describing the pair's other end.
const VETH_INFO_PEER: u16 = 1;

/// `IFF_UP` (`include/uapi/linux/if.h`): the interface is administratively
/// up. A newly created link starts without this flag — creating a veth pair
/// doesn't bring either end up, the way `ip link add` alone doesn't either
/// (confirmed the hard way: routing through a freshly created, still-down
/// link fails with `ENETDOWN`, "Network is down").
const IFF_UP: u32 = 0x1;

/// `struct ifinfomsg` (`include/uapi/linux/rtnetlink.h`), 16 bytes and
/// already 4-byte aligned: `ifi_family`, a padding byte, `ifi_type`,
/// `ifi_index`, `ifi_flags`, `ifi_change`. `ifi_change` is a mask: only the
/// bits set there are applied from `ifi_flags`. Creation (`index` `0`, the
/// kernel assigns one) and deletion (a real index, no flags) both leave it
/// zero.
///
/// Takes `u32` for `index`, not the kernel's signed `int` — matching
/// [`super::addr`]/[`super::route`]'s `ifindex`, since it's never negative.
fn ifinfomsg(index: u32, flags: u32, change: u32) -> [u8; 16] {
    let mut bytes = [0u8; 16];
    bytes[0] = AF_UNSPEC;
    bytes[4..8].copy_from_slice(&index.to_ne_bytes());
    bytes[8..12].copy_from_slice(&flags.to_ne_bytes());
    bytes[12..16].copy_from_slice(&change.to_ne_bytes());
    bytes
}

/// Builds an `RTM_NEWLINK` request that creates a veth pair: `host_ifname`
/// on the host, paired with `peer_ifname`.
///
/// `NLM_F_EXCL` makes the kernel fail the request instead of silently
/// reusing an existing link of the same name — a name collision here is a
/// bug worth surfacing, not something to paper over.
#[must_use]
pub fn create_veth_pair(host_ifname: &str, peer_ifname: &str, seq: u32) -> Vec<u8> {
    let flags = NLM_F_REQUEST | NLM_F_ACK | NLM_F_CREATE | NLM_F_EXCL;
    let mut msg = MessageBuilder::new(RTM_NEWLINK, flags, seq);
    msg.push_bytes(&ifinfomsg(0, 0, 0));
    msg.push_attr_str(IFLA_IFNAME, host_ifname);

    let linkinfo = msg.begin_nested(IFLA_LINKINFO);
    msg.push_attr_str(IFLA_INFO_KIND, "veth");
    let infodata = msg.begin_nested(IFLA_INFO_DATA);
    let peer = msg.begin_nested(VETH_INFO_PEER);
    // VETH_INFO_PEER's value is itself an ifinfomsg followed by attributes,
    // the same shape as the whole message's own payload — the kernel parses
    // the peer's description with the same code path it uses for a
    // top-level link.
    msg.push_bytes(&ifinfomsg(0, 0, 0));
    msg.push_attr_str(IFLA_IFNAME, peer_ifname);
    msg.finish_nested(&peer);
    msg.finish_nested(&infodata);
    msg.finish_nested(&linkinfo);

    msg.finish()
}

/// Builds an `RTM_DELLINK` request that deletes the link at `ifindex`.
/// Deleting either end of a veth pair deletes both — there's no separate
/// message for "and its peer too."
#[must_use]
pub fn delete_link(ifindex: u32, seq: u32) -> Vec<u8> {
    let flags = NLM_F_REQUEST | NLM_F_ACK;
    let mut msg = MessageBuilder::new(RTM_DELLINK, flags, seq);
    msg.push_bytes(&ifinfomsg(ifindex, 0, 0));
    msg.finish()
}

/// Builds an `RTM_NEWLINK` request that brings the link at `ifindex`
/// administratively up — what `ip link set <dev> up` does. Neither end of a
/// freshly created veth pair starts up on its own; routing through one
/// before this runs fails with `ENETDOWN`.
#[must_use]
pub fn set_link_up(ifindex: u32, seq: u32) -> Vec<u8> {
    let flags = NLM_F_REQUEST | NLM_F_ACK;
    let mut msg = MessageBuilder::new(RTM_NEWLINK, flags, seq);
    msg.push_bytes(&ifinfomsg(ifindex, IFF_UP, IFF_UP));
    msg.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::netlink::message::{AttributeIter, parse_header};

    /// Finds the first attribute of `attr_type` at the top level of `bytes`.
    fn find_attr(bytes: &[u8], attr_type: u16) -> &[u8] {
        AttributeIter::new(bytes)
            .find_map(|attr| match attr {
                Ok((t, value)) if t == attr_type => Some(value),
                _ => None,
            })
            .unwrap_or_else(|| panic!("attribute {attr_type} not found in {bytes:?}"))
    }

    /// A NUL-terminated string attribute's value, with the terminator
    /// stripped, as a plain string for assertions.
    fn as_str(value: &[u8]) -> &str {
        std::str::from_utf8(&value[..value.len() - 1]).unwrap()
    }

    mod create_veth_pair {
        use super::*;

        #[test]
        fn header_is_rtm_newlink_with_create_and_exclusive_flags() {
            let msg = create_veth_pair("narrows0", "tmp0", 5);

            let (header, _) = parse_header(&msg).unwrap();

            assert_eq!(header.msg_type, RTM_NEWLINK);
            assert_eq!(
                header.flags,
                NLM_F_REQUEST | NLM_F_ACK | NLM_F_CREATE | NLM_F_EXCL
            );
        }

        #[test]
        fn header_carries_the_given_sequence_number() {
            let msg = create_veth_pair("narrows0", "tmp0", 99);

            let (header, _) = parse_header(&msg).unwrap();

            assert_eq!(header.seq, 99);
        }

        #[test]
        fn message_length_is_a_multiple_of_four() {
            // Ragged names would expose a padding bug immediately.
            let msg = create_veth_pair("h", "peer-name-7", 0);

            assert_eq!(msg.len() % 4, 0);
        }

        #[test]
        fn top_level_ifname_is_the_host_side_name() {
            let msg = create_veth_pair("narrows0", "tmp0", 0);
            let (_, payload) = parse_header(&msg).unwrap();

            let ifname = find_attr(&payload[16..], IFLA_IFNAME);

            assert_eq!(as_str(ifname), "narrows0");
        }

        #[test]
        fn linkinfo_kind_is_veth() {
            let msg = create_veth_pair("narrows0", "tmp0", 0);
            let (_, payload) = parse_header(&msg).unwrap();
            let linkinfo = find_attr(&payload[16..], IFLA_LINKINFO);

            let kind = find_attr(linkinfo, IFLA_INFO_KIND);

            assert_eq!(as_str(kind), "veth");
        }

        #[test]
        fn peer_ifname_is_nested_under_info_data() {
            let msg = create_veth_pair("narrows0", "tmp0", 0);
            let (_, payload) = parse_header(&msg).unwrap();
            let linkinfo = find_attr(&payload[16..], IFLA_LINKINFO);
            let infodata = find_attr(linkinfo, IFLA_INFO_DATA);
            let peer = find_attr(infodata, VETH_INFO_PEER);

            // The peer's value starts with its own 16-byte ifinfomsg, then
            // its attributes — the same shape as the outer message.
            let peer_ifname = find_attr(&peer[16..], IFLA_IFNAME);

            assert_eq!(as_str(peer_ifname), "tmp0");
        }
    }

    mod delete_link {
        use super::*;

        #[test]
        fn header_is_rtm_dellink_with_no_create_flags() {
            let msg = delete_link(9, 2);

            let (header, _) = parse_header(&msg).unwrap();

            assert_eq!(header.msg_type, RTM_DELLINK);
            assert_eq!(header.flags, NLM_F_REQUEST | NLM_F_ACK);
        }

        #[test]
        fn ifinfomsg_carries_the_given_index() {
            let msg = delete_link(9, 0);
            let (_, payload) = parse_header(&msg).unwrap();

            let index = u32::from_ne_bytes(payload[4..8].try_into().unwrap());

            assert_eq!(index, 9);
        }

        #[test]
        fn message_length_is_a_multiple_of_four() {
            let msg = delete_link(9, 0);

            assert_eq!(msg.len() % 4, 0);
        }
    }

    mod set_link_up {
        use super::*;

        #[test]
        fn header_is_rtm_newlink_with_no_create_flags() {
            let msg = set_link_up(9, 3);

            let (header, _) = parse_header(&msg).unwrap();

            assert_eq!(header.msg_type, RTM_NEWLINK);
            assert_eq!(header.flags, NLM_F_REQUEST | NLM_F_ACK);
        }

        #[test]
        fn ifinfomsg_carries_the_given_index() {
            let msg = set_link_up(9, 0);
            let (_, payload) = parse_header(&msg).unwrap();

            let index = u32::from_ne_bytes(payload[4..8].try_into().unwrap());

            assert_eq!(index, 9);
        }

        #[test]
        fn sets_iff_up_in_both_flags_and_change_mask() {
            let msg = set_link_up(9, 0);
            let (_, payload) = parse_header(&msg).unwrap();

            let flags = u32::from_ne_bytes(payload[8..12].try_into().unwrap());
            let change = u32::from_ne_bytes(payload[12..16].try_into().unwrap());

            assert_eq!(flags, IFF_UP);
            assert_eq!(change, IFF_UP);
        }
    }
}
