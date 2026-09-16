# Netlink: an overview

This note is the bird's-eye view. The precise, per-field detail — struct layouts, byte order, why a particular flag is set — lives as doc comments in the code itself, next to the thing it explains (`crates/narrows-cni/src/netlink/`), per AGENTS.md §5's teaching-comment convention. Read this first for orientation; read the module docs when you need the specifics.

## What netlink is

`ip link add`, `ip addr add`, `ip route add` — everything the `ip` command does, it does by sending a message over an `AF_NETLINK` socket to the kernel. Netlink is a socket family, like `AF_INET`, except the "remote peer" is the kernel itself. A message says "create this link" or "add this route," and the kernel replies with an acknowledgement or an error.

Narrows needs this because the routed `/32` pod model (PLAN §6.1, [ADR-2](../adr/0002-routed-slash-32-pods.md)) is built entirely out of netlink operations: create a veth pair, assign the pod's address, add the pod's default route, add the host's route back to the pod, add a neighbour entry for the gateway.

## Why hand-rolled ([ADR-3](../adr/0003-netlink-implementation.md))

Three real options existed: hand-roll it, use the young `netlink-bindings` crate, or use the more established `rtnetlink` crate family. PLAN §1.2 puts "the first netlink calls" in the build-it-yourself column on purpose — this is exactly the kind of protocol where reading a byte layout off the kernel's own header and getting it right yourself teaches something a wrapper crate would hide. Phase 2's agent, with a broader and longer-lived netlink surface (watching for changes, not just one-shot requests), moves to `netlink-bindings` behind an internal trait.

## The wire format, briefly

Every message is: a fixed 16-byte header (`nlmsghdr`), then a payload of fixed-size structs and TLV ("type, length, value") attributes.

- **The header** says the message's total length, its type (`RTM_NEWLINK`, `RTM_NEWADDR`, ...), flags, and a sequence number the kernel echoes back.
- **An attribute** is a 4-byte header (a 16-bit length, a 16-bit type) followed by its value, padded out to a 4-byte boundary. Attributes can nest: `IFLA_LINKINFO` on a veth-creation request contains `IFLA_INFO_KIND` ("veth") and `IFLA_INFO_DATA`, which itself contains `VETH_INFO_PEER` describing the pair's other end.

Two byte-order gotchas, worth knowing before you're debugging a message that silently gets rejected:

1. **Integer fields are host-native byte order** (`to_ne_bytes`), not network byte order. That's the opposite of a `sockaddr_in`'s `sin_port`. Every mainstream Linux target this project cares about is little-endian, so getting this backwards produces a message that's simply wrong on the wire, with no compiler error to catch it.
2. **Address attribute *values*** — `IFA_LOCAL`, `RTA_DST`, `RTA_GATEWAY` — are different again: four raw octets in the order `10.244.1.7` reads left to right, which happens to be exactly what `Ipv4Addr::octets()` returns. An interface index in `RTA_OIF`, sitting right next to those in the same message, is a plain integer and follows rule 1 instead. Nothing in the format itself marks which rule applies to which attribute; only knowing what the attribute means does.

## What's built, and where

All of this is byte encoding and decoding only. No socket has been opened; nothing has touched a real interface, namespace, or route. That's deliberate — see the next section.

| Message | Module | What it builds |
| --- | --- | --- |
| `RTM_NEWLINK` | [`netlink::link`](../../crates/narrows-cni/src/netlink/link.rs) | Create the veth pair behind a pod's interface |
| `RTM_NEWADDR` | [`netlink::addr`](../../crates/narrows-cni/src/netlink/addr.rs) | Assign a pod's address to its interface |
| `RTM_NEWROUTE` ×2 | [`netlink::route`](../../crates/narrows-cni/src/netlink/route.rs) | The pod's default route to the gateway; the host's `/32` back to the pod |

The shared envelope layer — [`netlink::message`](../../crates/narrows-cni/src/netlink/message.rs) — builds and reads the header and attribute format itself, independent of any specific message type. It has both an encoder (`MessageBuilder`) and a decoder (`parse_header`, `AttributeIter`), because a later phase needs to parse the kernel's replies with the same TLV logic used to build requests, and because pairing every encode with a decode gives a much more reliable test than hand-typing a long hex literal for a message with several levels of nesting.

## What's still open

- **Moving the veth's peer into the pod's netns and renaming it** — another `RTM_NEWLINK` request, not yet built.
- **The gateway's neighbour entry** — `RTM_NEWNEIGH`, so the pod never needs to ARP for `169.254.1.1`.
- **The socket layer.** Everything above only produces `Vec<u8>`. Actually sending one of these messages needs real syscalls — `socket(AF_NETLINK, ...)`, `sendmsg`, and `setns` to enter a pod's namespace — which is the first genuinely `unsafe` code this project needs (AGENTS.md §3.2: syscalls are one of the few places `unsafe` is allowed, in a small dedicated module with a `# Safety` doc on every `unsafe fn`). Exercising that against a real namespace is also the first place this project needs the privileged test harness under `tests/netns/`, gated by AGENTS.md §1.2 — not something to build or run without that gate open.
