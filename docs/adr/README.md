# Architecture Decision Records

The list PLAN.md §15 asks for. Every one of these is **Proposed**, not Accepted: AGENTS.md §2 reserves accepting or rejecting an ADR for the human, so an agent drafting the write-up doesn't itself settle the decision. Where PLAN.md says a choice is already in use for a phase currently underway (ADR-3's hand-rolled netlink, for instance), that's ordinary incremental work staying inside an already-scoped phase, not a substitute for that sign-off.

| ADR | Title | Status |
| --- | --- | --- |
| [0001](0001-shim-agent-split.md) | Shim plus agent over a Unix socket, instead of a stateful plugin | Proposed |
| [0002](0002-routed-slash-32-pods.md) | Routed `/32` pod model, instead of a bridge | Proposed |
| [0003](0003-netlink-implementation.md) | Netlink implementation: hand-rolled now, `netlink-bindings` behind a trait later | Proposed |
| [0004](0004-custom-uds-framing.md) | Custom UDS framing, instead of gRPC | Proposed |
| [0005](0005-ip-based-policy.md) | IP-based NetworkPolicy enforcement, instead of identity-based | Proposed |
| [0006](0006-keep-kube-proxy.md) | Keep kube-proxy (nftables mode) until Phase 7 | Proposed |

Each file names the PLAN.md sections it draws from and updates from. When one of these is formally accepted or rejected, update its Status line and PLAN.md §15 together, with a Revision log entry (AGENTS.md §2).
