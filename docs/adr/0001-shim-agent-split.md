# ADR-1: Shim plus agent over a Unix socket, instead of a stateful plugin

- **Status:** Proposed — recommended by PLAN.md §3.1, §15; not yet formally accepted (AGENTS.md §2: accepting an ADR needs the human's sign-off, not an agent's).
- **Date:** 2026-09
- **Relates to:** PLAN.md §3.1 (Why a thin shim plus an agent), §4, §5, §12 Phase 1 exit criteria, §12 Phase 2 Build

## Context

A CNI runtime `exec`s the plugin binary fresh for every pod operation (PLAN §2.1). If the plugin held state itself — IPAM leases, open netlink sockets, a view of cluster topology — every invocation would need file locks for concurrency, would re-open sockets from scratch, and would have no memory of what earlier invocations did. Cilium and other production CNIs solve this by splitting the plugin in two: a thin binary the runtime execs, and a long-running daemon that owns all state, with the two talking over a Unix domain socket.

The cost of that split: if the daemon is down, no pod can get networking, and the runtime needs an honest way to learn that (the CNI spec's STATUS verb, added in 1.1, exists for exactly this).

## Decision

Build the split from the start of the *design*, but not from the start of the *code*. Phase 1 deliberately builds a single, "fat" plugin binary — `narrows-cni` does IPAM, netlink, everything — with no daemon and no socket. Phase 2 refactors that into `narrows-cni` (thin shim) plus `narrows-agent` (daemon), communicating over a UDS.

The reasoning for building it fat first, then splitting it, rather than starting with the split: the pain the split solves (state re-created per invocation, sockets re-opened per invocation, no cross-invocation memory) is only real once it's been felt. Building the fat version first is what makes Phase 2's refactor a genuine improvement to reason about, rather than a structure taken on faith (PLAN §3.1, "Learning path").

## Consequences

- Phase 1's `narrows-cni` will, for a while, hold responsibilities (IPAM state, netlink calls) that move to `narrows-agent` in Phase 2. That's expected churn, not a mistake to avoid.
- STATUS and GC, the two verbs the spec added specifically for the daemon-based pattern (delegated readiness reporting, and cleanup of resources orphaned by the daemon's own restarts), are deferred to Phase 2 for that reason — a fat plugin doesn't have the failure modes they exist to report on (PLAN §12 Phase 1, "Note on STATUS and GC").
- The UDS protocol itself is a separate decision (ADR-4).
