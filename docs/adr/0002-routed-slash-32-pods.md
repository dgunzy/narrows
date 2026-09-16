# ADR-2: Routed `/32` pod model, instead of a bridge

- **Status:** Proposed — recommended by PLAN.md §6.1, §15; not yet formally accepted (AGENTS.md §2).
- **Date:** 2026-09
- **Relates to:** PLAN.md §6.1 (Pod interface model: routed L3, no bridge), §6.2 (MTU), §6.4 (eBPF fast path), §12 Phase 1

## Context

The traditional Linux CNI shape puts a Linux bridge on the host, with every pod's veth attached to it, and pods talking L2 (ARP, MAC learning) to reach their gateway and each other on the same node. That's what Docker's default bridge network and early Kubernetes CNIs (kubenet, the original bridge plugin) do.

The alternative — used by Cilium's native routing mode and by netkit's L3 design — is a routed model: no bridge, no L2 segment at all. Each pod gets a single `/32` address on `eth0`, a default route to a fixed gateway address, and a static ARP entry for that gateway so the pod never needs to actually resolve it. The host side of the veth gets a `/32` route straight to the pod.

## Decision

Use the routed `/32` model, not a bridge. Every pod's connectivity reduces to routes: the kernel forwards on `ifindex` and destination prefix, with no MAC learning, no ARP flooding, and no bridge netfilter hooks (`br_netfilter`) to reason about.

The gateway every pod's default route points to is a single fixed link-local address, `169.254.1.1` — the same choice Calico makes — rather than something per-node or per-pod that would need tracking (PLAN §6.1 revision, 2026-09-17).

## Consequences

- **Trade-off accepted:** `ip_forward` and `rp_filter` need explicit sysctl management per interface, since nothing about this model works without the kernel forwarding between the pod's veth and the rest of the host (PLAN §14.6).
- **Why this shape pays off later:** "every pod is just a route" maps directly onto an eBPF lookup once Phase 4's fast path exists — a routing decision and a map lookup are the same shape, which a bridge's L2 forwarding wouldn't be (PLAN §6.1, §6.4).
- **Prior art this tracks:** it's the same interface shape as netkit's L3 mode, which Cilium recommends specifically because it removes ARP handling from the picture (PLAN §6.1, §6.5).
- **What this doesn't decide:** how packets cross *between* nodes (host-gw vs. VXLAN, PLAN §6.3) or how the eBPF fast path avoids the NAT-bypass hazard (PLAN §6.4) — both are separate, later decisions this ADR doesn't cover.
