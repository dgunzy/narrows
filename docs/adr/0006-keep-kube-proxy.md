# ADR-6: Keep kube-proxy (nftables mode) until Phase 7

- **Status:** Proposed — recommended by PLAN.md §2.3, §15; not yet formally accepted (AGENTS.md §2).
- **Date:** 2026-09
- **Relates to:** PLAN.md §1.1 (Non-goals), §2.3 (What stays outside Narrows), §6.4 (the NAT hazard this decision sidesteps for now), §7 (stretch: kube-proxy replacement), `deploy/kind/narrows.yaml`

## Context

A CNI project can, in principle, replace kube-proxy's Service-VIP handling with its own datapath — Cilium's eBPF kube-proxy replacement is the reference example. Doing that correctly means getting DNAT/un-DNAT right for every Service backend, including the specific failure mode PLAN §6.4 calls out: an eBPF fast-path redirect that bypasses host netfilter can skip the conntrack-driven un-DNAT a Service reply needs, silently breaking every Service-backed connection.

## Decision

Leave kube-proxy in place, running in nftables mode, for the entire span of Phases 1 through 6. Service VIP handling is explicitly out of scope until Phase 7 at the earliest (PLAN §1.1, §2.3).

`deploy/kind/narrows.yaml` already reflects this: `kubeProxyMode: "nftables"` is set, and the default CNI is disabled while kube-proxy is left running.

## Consequences

- **What this defers, not avoids:** the NAT-and-conntrack-bypass hazard (PLAN §6.4) is still a real correctness problem the eBPF fast path (Phase 4) has to handle for pod-to-pod traffic that happens to be NATed — this ADR only takes *Service* traffic off the table until Phase 7, it doesn't make the general hazard disappear.
- **nftables over iptables:** nftables mode is GA since Kubernetes 1.33 but not yet the default, so it has to be requested explicitly — which `deploy/kind/narrows.yaml` does. kind also sets `tcp_be_liberal` in this mode to work around TCP conntrack strictness on older kernels (PLAN §14.5); other test clusters need to match that by hand.
- **Phase 7 stretch item, not a Phase 1–6 goal:** "kube-proxy replacement via socket-level load balancing" is listed as stretch work (PLAN §12 Phase 7) specifically because it also removes the NAT hazard at its root — translating at `connect()` time means packets already carry real backend IPs, so there's no DNAT to un-DNAT. That's a strong enough motivation that this ADR should be revisited once Phase 4's eBPF work is further along, not treated as settled for the life of the project.
