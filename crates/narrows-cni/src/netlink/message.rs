//! The netlink envelope (`nlmsghdr`) and the TLV attribute format every
//! rtnetlink payload is built from.
//!
//! # Byte order
//!
//! Every integer here is **host-native byte order**, not network byte
//! order — the opposite of a `sockaddr_in`'s `sin_port`/`sin_addr`. Get it
//! backwards and a message comes out wrong on the wire with no compiler
//! error to catch it. Always `to_ne_bytes`/`from_ne_bytes`, never `_be_`.
//!
//! # Alignment
//!
//! Every field and attribute is padded to a 4-byte boundary. An attribute's
//! `rta_len` holds the *unpadded* size, so a reader must round it up itself
//! to find the next attribute. [`MessageBuilder`] and [`AttributeIter`]
//! agree on this so what one writes, the other reads back correctly.

use std::fmt;

/// Every netlink field and attribute is aligned to this boundary.
pub const ALIGN_TO: usize = 4;

/// Rounds `len` up to the next multiple of [`ALIGN_TO`].
#[must_use]
pub const fn align(len: usize) -> usize {
    (len + ALIGN_TO - 1) & !(ALIGN_TO - 1)
}

/// `NLM_F_REQUEST`: this message is a request, not a notification or reply.
pub const NLM_F_REQUEST: u16 = 0x01;
/// `NLM_F_ACK`: ask the kernel for an explicit acknowledgement.
pub const NLM_F_ACK: u16 = 0x04;
/// `NLM_F_EXCL`: fail instead of updating if the object already exists.
pub const NLM_F_EXCL: u16 = 0x200;
/// `NLM_F_CREATE`: create the object if it doesn't exist.
pub const NLM_F_CREATE: u16 = 0x400;

/// `sizeof(struct nlmsghdr)`: 4 `u32`/`u16` fields, already 4-byte aligned.
const HEADER_LEN: usize = 16;

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_ne_bytes([bytes[offset], bytes[offset + 1]])
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_ne_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

/// Builds one netlink message: the `nlmsghdr` envelope, then a payload of
/// fixed-size structs and TLV attributes.
///
/// Usage: [`new`](Self::new) reserves the header; [`push_bytes`](Self::push_bytes)
/// appends a fixed struct like `ifinfomsg`; [`push_attr`](Self::push_attr) /
/// [`push_attr_str`](Self::push_attr_str) append one attribute;
/// [`begin_nested`](Self::begin_nested) / [`finish_nested`](Self::finish_nested)
/// wrap a group of attributes in a container like `IFLA_LINKINFO` — its length
/// isn't known until its contents are written, so `begin_nested` reserves
/// space and `finish_nested` fills it in later, the same trick `finish` uses
/// for the header's own length.
#[derive(Debug)]
pub struct MessageBuilder {
    buf: Vec<u8>,
}

impl MessageBuilder {
    /// Starts a message of the given type and flags.
    ///
    /// `seq` is the caller's request sequence number, echoed back in the
    /// kernel's reply so a caller matching replies to requests can tell them
    /// apart. `nlmsg_pid` is left `0`: a request may leave it for the kernel
    /// to fill in, and this builder never opens the socket that would need
    /// to set it to anything else.
    #[must_use]
    pub fn new(msg_type: u16, flags: u16, seq: u32) -> Self {
        let mut buf = vec![0u8; HEADER_LEN];
        buf[4..6].copy_from_slice(&msg_type.to_ne_bytes());
        buf[6..8].copy_from_slice(&flags.to_ne_bytes());
        buf[8..12].copy_from_slice(&seq.to_ne_bytes());
        Self { buf }
    }

    /// Appends a fixed-size struct with no TLV wrapper, such as an
    /// `ifinfomsg`.
    ///
    /// Every struct this crate builds is already a multiple of
    /// [`ALIGN_TO`] bytes, by construction (`ifinfomsg` is 16), so nothing
    /// here needs to pad `bytes` itself.
    pub fn push_bytes(&mut self, bytes: &[u8]) {
        debug_assert_eq!(bytes.len() % ALIGN_TO, 0, "unaligned fixed-size struct");
        self.buf.extend_from_slice(bytes);
    }

    /// Appends one TLV attribute: an `rta_len`/`rta_type` header, then
    /// `value`, padded to a 4-byte boundary.
    pub fn push_attr(&mut self, attr_type: u16, value: &[u8]) {
        let start = self.buf.len();
        self.buf.extend_from_slice(&[0u8; 4]);
        self.buf.extend_from_slice(value);
        self.patch_attr_header(start, attr_type);
        self.pad_to_align();
    }

    /// Appends a NUL-terminated string attribute, such as `IFLA_IFNAME`.
    ///
    /// The kernel's own attribute helpers (`nla_put_string`) always add the
    /// terminator; an attribute built without one is read back one byte
    /// short of the name it should hold.
    pub fn push_attr_str(&mut self, attr_type: u16, value: &str) {
        let mut bytes = Vec::with_capacity(value.len() + 1);
        bytes.extend_from_slice(value.as_bytes());
        bytes.push(0);
        self.push_attr(attr_type, &bytes);
    }

    /// Opens a container attribute, such as `IFLA_LINKINFO`, whose value is
    /// itself more attributes (or, for `VETH_INFO_PEER`, a struct followed by
    /// attributes). Its contents go through further calls on `self`; pass
    /// the returned handle to [`MessageBuilder::finish_nested`] once they're
    /// written.
    pub fn begin_nested(&mut self, attr_type: u16) -> NestedAttr {
        let start = self.buf.len();
        self.buf.extend_from_slice(&[0u8; 4]);
        NestedAttr { start, attr_type }
    }

    /// Closes a container opened with [`MessageBuilder::begin_nested`],
    /// now that its contents (and so its length) are known.
    pub fn finish_nested(&mut self, nested: &NestedAttr) {
        self.patch_attr_header(nested.start, nested.attr_type);
        self.pad_to_align();
    }

    /// Fills in the header's total length and returns the finished message.
    #[must_use]
    pub fn finish(mut self) -> Vec<u8> {
        let len = self.buf.len();
        debug_assert!(
            u32::try_from(len).is_ok(),
            "message longer than 4 GiB: {len}"
        );
        #[expect(
            clippy::cast_possible_truncation,
            reason = "checked by the debug_assert above; real messages here are a few hundred bytes at most"
        )]
        let len = len as u32;
        self.buf[0..4].copy_from_slice(&len.to_ne_bytes());
        self.buf
    }

    /// Writes `rta_len` (the buffer's current length minus `start`) and
    /// `rta_type` into the 4-byte header reserved at `start`.
    fn patch_attr_header(&mut self, start: usize, attr_type: u16) {
        let len = self.buf.len() - start;
        debug_assert!(
            u16::try_from(len).is_ok(),
            "attribute value too large: {len}"
        );
        #[expect(
            clippy::cast_possible_truncation,
            reason = "checked by the debug_assert above; attribute values here are tiny"
        )]
        let len = len as u16;
        self.buf[start..start + 2].copy_from_slice(&len.to_ne_bytes());
        self.buf[start + 2..start + 4].copy_from_slice(&attr_type.to_ne_bytes());
    }

    fn pad_to_align(&mut self) {
        self.buf.resize(align(self.buf.len()), 0);
    }
}

/// A container attribute opened by [`MessageBuilder::begin_nested`] and not
/// yet closed. Carries no data of its own, only the bookkeeping
/// [`MessageBuilder::finish_nested`] needs.
#[derive(Debug)]
#[must_use = "an attribute opened with begin_nested must be closed with finish_nested, or its length is never filled in"]
pub struct NestedAttr {
    start: usize,
    attr_type: u16,
}

/// A parsed message header: the fixed fields every netlink message starts
/// with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessageHeader {
    /// The message's total length, header included.
    pub len: u32,
    /// Which kind of message this is, such as `RTM_NEWLINK`.
    pub msg_type: u16,
    /// `NLM_F_*` flags.
    pub flags: u16,
    /// The requester's sequence number, echoed back in replies.
    pub seq: u32,
    /// The sending socket's port ID.
    pub pid: u32,
}

/// Why a byte buffer couldn't be read as netlink.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetlinkError {
    /// Fewer than 16 bytes: not enough for even the header.
    TooShortForHeader,
    /// The header's `nlmsg_len` doesn't fit the buffer it was found in.
    LengthOutOfBounds,
    /// An attribute's `rta_len` is too small to hold its own header, or runs
    /// past the end of the bytes it was found in.
    TruncatedAttribute,
}

impl fmt::Display for NetlinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::TooShortForHeader => "buffer is shorter than a netlink message header",
            Self::LengthOutOfBounds => "nlmsg_len does not fit the given buffer",
            Self::TruncatedAttribute => "an attribute's length runs past the end of its container",
        })
    }
}

impl std::error::Error for NetlinkError {}

/// Parses the header off the front of `bytes` and returns it along with the
/// payload that follows, up to `header.len`.
///
/// # Errors
///
/// Returns [`NetlinkError::TooShortForHeader`] if `bytes` is under 16 bytes,
/// and [`NetlinkError::LengthOutOfBounds`] if the header's own `nlmsg_len`
/// is smaller than the header itself or larger than `bytes`.
pub fn parse_header(bytes: &[u8]) -> Result<(MessageHeader, &[u8]), NetlinkError> {
    if bytes.len() < HEADER_LEN {
        return Err(NetlinkError::TooShortForHeader);
    }
    let len = read_u32(bytes, 0);
    let total = usize::try_from(len).map_err(|_| NetlinkError::LengthOutOfBounds)?;
    if total < HEADER_LEN || total > bytes.len() {
        return Err(NetlinkError::LengthOutOfBounds);
    }
    let header = MessageHeader {
        len,
        msg_type: read_u16(bytes, 4),
        flags: read_u16(bytes, 6),
        seq: read_u32(bytes, 8),
        pid: read_u32(bytes, 12),
    };
    Ok((header, &bytes[HEADER_LEN..total]))
}

/// Walks a run of TLV attributes: a message's payload after any leading
/// fixed-size struct, or the inside of a container attribute such as
/// `IFLA_LINKINFO`.
#[derive(Debug, Clone)]
pub struct AttributeIter<'a> {
    remaining: &'a [u8],
}

impl<'a> AttributeIter<'a> {
    /// Starts walking the attributes in `bytes`.
    #[must_use]
    pub const fn new(bytes: &'a [u8]) -> Self {
        Self { remaining: bytes }
    }
}

impl<'a> Iterator for AttributeIter<'a> {
    /// The attribute's type and value, or the error that stopped iteration.
    /// An error ends the walk: `next()` returns `None` on every call after.
    type Item = Result<(u16, &'a [u8]), NetlinkError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining.is_empty() {
            return None;
        }
        if self.remaining.len() < 4 {
            self.remaining = &[];
            return Some(Err(NetlinkError::TruncatedAttribute));
        }
        let rta_len = usize::from(read_u16(self.remaining, 0));
        let rta_type = read_u16(self.remaining, 2);
        if rta_len < 4 || rta_len > self.remaining.len() {
            self.remaining = &[];
            return Some(Err(NetlinkError::TruncatedAttribute));
        }
        let value = &self.remaining[4..rta_len];
        let consumed = align(rta_len).min(self.remaining.len());
        self.remaining = &self.remaining[consumed..];
        Some(Ok((rta_type, value)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod align {
        use super::*;

        #[test]
        fn leaves_a_multiple_of_four_unchanged() {
            assert_eq!(align(8), 8);
        }

        #[test]
        fn rounds_up_to_the_next_multiple_of_four() {
            assert_eq!(align(5), 8);
        }

        #[test]
        fn zero_stays_zero() {
            assert_eq!(align(0), 0);
        }
    }

    /// These two exact-byte tests pin down the wire format precisely, so
    /// they only run where a literal little-endian byte array is the right
    /// expectation — every target this project ships to (PLAN §2.2).
    /// Elsewhere, the round-trip and structural tests below still cover
    /// correctness; they only rely on `to_ne_bytes`/`from_ne_bytes`
    /// agreeing with themselves, not on a specific byte order.
    #[cfg(target_endian = "little")]
    mod exact_bytes {
        use super::*;

        #[test]
        fn message_with_one_flat_attribute() {
            let mut msg = MessageBuilder::new(5, 1, 7);
            msg.push_attr(3, b"ok\0");

            // Header (16B): len=24, type=5, flags=1, seq=7, pid=0.
            // Attr (8B): rta_len=7 (header+"ok\0", unpadded), rta_type=3,
            // value "ok\0", then 1 pad byte to reach the 4-byte boundary.
            #[rustfmt::skip]
            let expected: &[u8] = &[
                24, 0, 0, 0,  5, 0,  1, 0,  7, 0, 0, 0,  0, 0, 0, 0,
                 7, 0,  3, 0,  b'o', b'k', 0, 0,
            ];
            assert_eq!(msg.finish(), expected);
        }

        #[test]
        fn message_with_a_nested_attribute_holding_two_children() {
            let mut msg = MessageBuilder::new(1, 0, 0);
            let nested = msg.begin_nested(10);
            msg.push_attr(1, &[0xAA]);
            msg.push_attr(2, &[]);
            msg.finish_nested(&nested);

            #[rustfmt::skip]
            let expected: &[u8] = &[
                // Header (16B): len=32, type=1, flags=0, seq=0, pid=0.
                32, 0, 0, 0,  1, 0,  0, 0,  0, 0, 0, 0,  0, 0, 0, 0,
                // Nested attr header (4B): rta_len=16 (4 + 8 + 4), type=10.
                16, 0,  10, 0,
                // Child A (8B): rta_len=5 (header + 1 byte), type=1, value
                // 0xAA, then 3 pad bytes.
                5, 0,  1, 0,  0xAA, 0, 0, 0,
                // Child B (4B): rta_len=4 (header only, empty value), type=2.
                4, 0,  2, 0,
            ];
            assert_eq!(msg.finish(), expected);
        }
    }

    mod round_trip {
        use super::*;

        #[test]
        fn header_fields_survive_parse() {
            let msg = MessageBuilder::new(16, NLM_F_REQUEST | NLM_F_ACK, 42).finish();

            let (header, _) = parse_header(&msg).unwrap();

            assert_eq!(
                header,
                MessageHeader {
                    len: u32::try_from(msg.len()).unwrap(),
                    msg_type: 16,
                    flags: NLM_F_REQUEST | NLM_F_ACK,
                    seq: 42,
                    pid: 0,
                }
            );
        }

        #[test]
        fn flat_attribute_survives_parse() {
            let mut msg = MessageBuilder::new(0, 0, 0);
            msg.push_attr(9, b"hello");
            let msg = msg.finish();

            let (_, payload) = parse_header(&msg).unwrap();
            let attrs: Vec<_> = AttributeIter::new(payload)
                .collect::<Result<_, _>>()
                .unwrap();

            assert_eq!(attrs, [(9, b"hello".as_slice())]);
        }

        #[test]
        fn nested_attribute_survives_parse() {
            let mut msg = MessageBuilder::new(0, 0, 0);
            let outer = msg.begin_nested(20);
            msg.push_attr(1, b"a");
            msg.push_attr(2, b"bb");
            msg.finish_nested(&outer);
            let msg = msg.finish();

            let (_, payload) = parse_header(&msg).unwrap();
            let mut attrs = AttributeIter::new(payload);
            let (outer_type, outer_value) = attrs.next().unwrap().unwrap();
            assert!(
                attrs.next().is_none(),
                "expected only one top-level attribute"
            );

            let inner: Vec<_> = AttributeIter::new(outer_value)
                .collect::<Result<_, _>>()
                .unwrap();

            assert_eq!(outer_type, 20);
            assert_eq!(inner, [(1, b"a".as_slice()), (2, b"bb".as_slice())]);
        }

        #[test]
        fn fixed_struct_and_attribute_coexist() {
            let mut msg = MessageBuilder::new(0, 0, 0);
            msg.push_bytes(&[1, 2, 3, 4]);
            msg.push_attr(1, b"x");
            let msg = msg.finish();

            let (_, payload) = parse_header(&msg).unwrap();
            // The fixed struct isn't an attribute, so the caller must skip it
            // itself before walking the rest as TLVs.
            let attrs: Vec<_> = AttributeIter::new(&payload[4..])
                .collect::<Result<_, _>>()
                .unwrap();

            assert_eq!(attrs, [(1, b"x".as_slice())]);
        }
    }

    mod parse_header {
        use super::*;

        #[test]
        fn rejects_buffer_shorter_than_header() {
            assert_eq!(parse_header(&[0; 15]), Err(NetlinkError::TooShortForHeader));
        }

        #[test]
        fn rejects_length_smaller_than_header() {
            let mut bytes = [0u8; 16];
            bytes[0..4].copy_from_slice(&8u32.to_ne_bytes());

            assert_eq!(parse_header(&bytes), Err(NetlinkError::LengthOutOfBounds));
        }

        #[test]
        fn rejects_length_past_end_of_buffer() {
            let mut bytes = [0u8; 16];
            bytes[0..4].copy_from_slice(&100u32.to_ne_bytes());

            assert_eq!(parse_header(&bytes), Err(NetlinkError::LengthOutOfBounds));
        }
    }

    mod attribute_iter {
        use super::*;

        #[test]
        fn empty_input_yields_nothing() {
            assert_eq!(AttributeIter::new(&[]).next(), None);
        }

        #[test]
        fn fewer_than_four_bytes_is_truncated() {
            assert_eq!(
                AttributeIter::new(&[1, 2, 3]).next(),
                Some(Err(NetlinkError::TruncatedAttribute))
            );
        }

        #[test]
        fn rta_len_past_end_is_truncated() {
            let bytes = [0xFFu8, 0xFF, 0, 0];

            assert_eq!(
                AttributeIter::new(&bytes).next(),
                Some(Err(NetlinkError::TruncatedAttribute))
            );
        }

        #[test]
        fn stops_after_an_error() {
            let mut iter = AttributeIter::new(&[1, 2, 3]);
            iter.next();

            assert_eq!(iter.next(), None);
        }
    }
}
