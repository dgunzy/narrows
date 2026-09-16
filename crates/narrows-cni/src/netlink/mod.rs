//! Hand-rolled rtnetlink message encoding, and the socket that sends it
//! (PLAN §3.2, ADR-3(a)).
//!
//! rtnetlink is the protocol behind the `ip` command: a socket family
//! (`AF_NETLINK`) for asking the kernel to create links, addresses, and
//! routes — a fixed header, then TLV ("type, length, value") attributes.
//!
//! [`message`], [`link`], [`addr`], and [`route`] build and read byte
//! buffers only, no `unsafe`. `socket` (Linux-only; empty elsewhere, see its
//! own docs) is where a message reaches the kernel: opening an `AF_NETLINK`
//! socket and sending on it, this crate's first `unsafe` code (AGENTS.md
//! §3.2). Actually modifying a real namespace needs `CAP_NET_ADMIN`; that's
//! `tests/netns/`, run only when the human asks (AGENTS.md §1.2).
//!
//! Every struct layout and constant here is from the kernel's own
//! `include/uapi/linux/{netlink,rtnetlink,if_link,if_addr,veth}.h`, not
//! guessed.

pub mod addr;
pub mod link;
pub mod message;
pub mod route;
pub mod socket;

pub use message::{AttributeIter, MessageBuilder, MessageHeader, NestedAttr, NetlinkError};
