# ADR-5: IP-based NetworkPolicy enforcement, instead of identity-based

- **Status:** Proposed — recommended by PLAN.md §7, §15; not yet formally accepted (AGENTS.md §2).
- **Date:** 2026-09
- **Relates to:** PLAN.md §7 (NetworkPolicy, Phase 5), §7.2 (Prior art worth reading)

## Context

Two broad strategies exist for enforcing Kubernetes NetworkPolicy in a CNI's datapath:

- **IP-based:** compile each policy into rules keyed by peer IP prefix, protocol, and port, evaluated per packet.
- **Identity-based** (Cilium's approach): assign every endpoint a stable numeric identity derived from its labels, and enforce policy against that identity rather than against the IP that happens to be attached to it at the moment. This survives pod IP churn without recompiling policy, at the cost of a separate identity-allocation and distribution system.

## Decision

IP-based for Phase 5. Each NetworkPolicy compiles to per-endpoint ingress and egress maps keyed by peer prefix (an LPM trie lookup), protocol, and port; endpoints selected by no policy skip the lookup entirely (PLAN §7.1).

## Consequences

- **Accepted trade-off:** a label change that alters an endpoint's IP-relevant policy set requires recompiling the affected endpoints' maps, rather than being absorbed by an identity layer. PLAN §7.1 already designs for this being cheap — "the compiler is a pure function (policies, pods) → map contents, so it is heavily unit-testable" — but it's still real recompilation work identity-based enforcement wouldn't need.
- **Revisit condition, stated in the plan itself:** "revisit if label churn cost shows up in benchmarks" (PLAN §15). This ADR is explicitly provisional on that measurement, not a permanent choice.
- **Correctness oracle:** `kube-network-policies` (kubernetes-sigs), which enforces policy in userspace via NFQUEUE, is named in the plan as a project to borrow selectivity ideas from and to cross-check tricky NetworkPolicy behaviour against (PLAN §7.2, §12 Phase 5 exit criteria) — not as an implementation to copy, but as a way to catch places where Narrows's own semantics might be wrong.
