# ADR-4: Custom UDS framing, instead of gRPC

- **Status:** Proposed — recommended by PLAN.md §8.3, §15; not yet formally accepted (AGENTS.md §2).
- **Date:** 2026-09
- **Relates to:** PLAN.md §3.1 (shim/agent split, ADR-1), §8 (Observability Plane), §8.3 (Agent side — Protocol), §12 Phase 2 Build

## Context

Two things inside Narrows need a wire protocol over a Unix domain socket: the shim talking to the agent (ADR-1), and the agent's flow server streaming events out to `narrows-tui` (PLAN §8.3). gRPC is the obvious off-the-shelf answer for "a typed protocol over a socket" and is what a lot of production systems reach for.

## Decision

Don't use gRPC. Use a small custom framing instead: length-prefixed binary frames over the UDS, with a version byte and a small, fixed request set (subscribe with a filter, snapshot tables, stats).

## Consequences

- **The accepted cost:** no free multi-node relay. gRPC would have given Narrows a path to a multi-node flow relay (PLAN §7 stretch list) largely for free; a custom frame format means that, if it's ever wanted, it gets built by hand too.
- **What this buys instead:** keeps the dependency budget small (PLAN §9) — no protobuf toolchain, no generated code, no gRPC runtime — for a protocol whose actual shape (subscribe, snapshot, stats) is simple enough not to need one. It's also consistent with the project's build-it-yourself philosophy for "the agent–shim protocol" specifically (PLAN §1.2).
- This is a narrower decision than ADR-1: ADR-1 is *that* the shim and agent talk over a UDS at all; this ADR is *what* goes over that socket once they do.
