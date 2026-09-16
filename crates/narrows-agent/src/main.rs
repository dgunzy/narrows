//! `narrows-agent`: the per-node daemon (PLAN §5).
//!
//! Stub only. In Phase 2 it will own all node state: IPAM, netlink, the eBPF
//! lifecycle, policy, and flows. Phase 1 is a fat plugin inside `narrows-cni`
//! on purpose, so the reasons for the shim/agent split are felt first (PLAN §3.1).

fn main() {}
