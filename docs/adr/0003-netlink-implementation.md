# ADR-3: Netlink implementation

- **Status:** Proposed — recommended by PLAN.md §15; not yet formally accepted (AGENTS.md §2). Option (a) is already in active use for Phase 1's message-encoding layer (see "Current state" below); that's normal incremental development staying inside an already-scoped phase, not a substitute for the human's formal acceptance of the ADR itself.
- **Date:** 2026-09
- **Relates to:** PLAN.md §1.2 (Library philosophy), §9 (Dependency Budget), §12 Phase 1/2, §14.8, `crates/narrows-cni/src/netlink/`

## Context

Narrows needs to talk rtnetlink: create veth pairs, assign addresses, add routes, add neighbour entries, and — from Phase 2 on — watch for changes and clean up on teardown. Three ways to get there were on the table:

- **(a) Hand-roll** the specific rtnetlink messages Narrows needs, directly against the kernel's wire format. Maximum learning, smallest dependency surface, but every message type is work the crate ecosystem has already done once.
- **(b) `netlink-bindings`.** Generated from the kernel's own YAML netlink specs, with tested coverage for rt-link, rt-route, rt-addr, conntrack, nftables, and tc. Young (0.3.x at the time this was written), and its rt-neigh family wasn't yet tested.
- **(c) The rust-netlink family** (`rtnetlink` and friends). More established, but at last check its conntrack support lived in a fork awaiting upstream merge, not upstream itself.

PLAN §1.2 puts "the first netlink calls" in the build-it-yourself column specifically because encoding the wire format by hand is where the learning is — see the module docs in `crates/narrows-cni/src/netlink/` for what that means concretely (byte order, TLV alignment, nested attributes).

## Decision

**(a)** for Phase 1: hand-roll the handful of messages the fat plugin needs. **(b)**, behind an internal trait, from Phase 2 on, once the agent needs a broader and longer-lived netlink surface (watching for changes, not just issuing one-shot requests) than hand-rolling comfortably covers.

## Current state

As of this writing, the Phase 1 encoding layer covers, all as pure byte encoding with no socket opened yet:

| Message | Module | Purpose |
| --- | --- | --- |
| `RTM_NEWLINK` (veth pair) | `netlink::link` | Create the veth pair behind a pod's interface |
| `RTM_NEWADDR` | `netlink::addr` | Assign a pod's address to its interface |
| `RTM_NEWROUTE` ×2 | `netlink::route` | The pod's default route and the host's `/32` back to it |

Still open: moving the veth's peer into the pod's netns and renaming it, the gateway's neighbour entry (`RTM_NEWNEIGH`), and the socket/syscall layer (`socket`, `sendmsg`, `setns`) that would actually send any of this — the first genuinely `unsafe` code in this project, gated by AGENTS.md §1.2 for testing against a real namespace.

## Consequences

- Keeping the netlink layer behind an internal trait from Phase 2 (PLAN §14.8) means the eventual switch to (b) doesn't ripple through the agent's other subsystems — they depend on the trait, not on which crate implements it.
- If (b)'s rt-neigh coverage or general maturity hasn't improved by the time Phase 2 needs it, this ADR should be revisited rather than assumed settled — that's exactly the kind of "young dependency" risk PLAN §14.8 flags.
