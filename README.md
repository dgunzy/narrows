# Narrows

[![CI](https://github.com/dgunzy/narrows/actions/workflows/ci.yml/badge.svg)](https://github.com/dgunzy/narrows/actions/workflows/ci.yml)

A Kubernetes CNI plugin written in Rust, with a terminal UI for watching packet flows.

> **This is a learning project. Do not use it in production.** It exists to understand how pod networking works by building it, one layer at a time.

## Concepts at play

- **CNI (Container Network Interface).** The contract between a container runtime (containerd, CRI-O) and a network plugin. For each pod, the runtime executes the plugin binary, passes parameters in `CNI_*` environment variables and JSON on stdin, and reads a JSON result from stdout. The verbs are ADD, DEL, CHECK, STATUS, GC, and VERSION.
- **Network namespaces and veth pairs.** Each pod has its own network stack. A veth pair is a virtual cable with one end inside the pod and one on the host.
- **Routed `/32` pods.** Each pod gets a single address and a host route to it, with no bridge. Every pod is just a route.
- **rtnetlink.** The kernel's socket API for configuring links, addresses, and routes. It's what the `ip` command uses under the hood.
- **IPAM.** Handing out pod IPs from the node's pod CIDR, without leaks or unsafe reuse.
- **Shim plus agent.** A tiny plugin binary forwards requests over a Unix socket to a long-running per-node daemon that owns all state.
- **Cross-node traffic.** Plain routes between nodes (host-gw) first, then a VXLAN overlay.
- **eBPF and tcx.** Small verified programs attached to interfaces in the kernel, for a fast path, NetworkPolicy enforcement, and flow and drop events streamed over a ring buffer.
- **netkit.** A newer alternative to veth, built for eBPF-driven datapaths.

The full design and roadmap are in [PLAN.md](PLAN.md), including a phase-by-phase [Revision log](PLAN.md#revision-log) at the bottom recording what changed and why. The dev setup is in [docs/notes/dev-setup.md](docs/notes/dev-setup.md); a deeper walkthrough of the netlink work specifically is in [docs/notes/netlink.md](docs/notes/netlink.md). Architecture decisions are recorded in [docs/adr/](docs/adr/), currently all **Proposed** rather than formally accepted.

## Status

Phase 0 (lab and harness) and the early slices of Phase 1 (single-node fat plugin) are underway: CNI request parsing, the VERSION verb, IPAM, and hand-rolled netlink message encoding exist; none of it yet sends a real message to the kernel. See PLAN.md's Phase 1 status note (§12) for the current detail.

```sh
cargo xtask ci   # fmt, clippy, test, doc — the same checks CI runs
```

## License

[Apache License 2.0](LICENSE)
