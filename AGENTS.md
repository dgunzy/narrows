# AGENTS.md: Narrows

Narrows is a learning-focused Kubernetes CNI written in Rust, with an optional flow-visualization TUI. The design lives in `PLAN.md`.

These instructions apply to every coding agent working in this repo. `CLAUDE.md` imports this file, so keep shared guidance here and only tool-specific notes there.

---

## 1. Hard rules

Never break these, even if a task seems to require it. If one blocks you, stop and ask.

### 1.1 No git write operations

**Allowed (read-only):**
- `status`, `diff`, `log`, `show`, `blame`, `grep`
- `ls-files`, `rev-parse`, `branch --show-current`, `remote -v`

**Not allowed:** anything that changes the index, HEAD, refs, stash, config, or remotes, or that talks to a remote. That includes:
- `add`, `commit`, `push`, `pull`, `fetch`, `clone`
- `merge`, `rebase`, `reset`, `restore`, `checkout`, `switch`
- `stash`, `tag`, `cherry-pick`, `revert`, `am`, `apply`
- `rm`, `mv`, `clean`
- config writes, and branch, remote, submodule, or worktree changes
- `gh` write actions: creating or merging PRs, issues, releases, workflow runs, and `gh api` mutations

**How to comply:**
- Leave all changes uncommitted in the working tree. The human reviews and commits.
- Don't work around this rule with aliases, plumbing commands, scripts, or other tools that write to `.git/`.

### 1.2 Don't touch host networking or the kernel

On the development machine, never:
- run `ip`, `nft`, `iptables`, `tc`, `sysctl`, `bpftool`, `conntrack`, `modprobe`, or `sudo`
- create network namespaces or load eBPF programs
- run privileged tests (anything under `tests/netns/`, or `cargo xtask` privileged targets)

The exception is when the human explicitly asks for it in the current session. That work belongs in the kind cluster or a VM (PLAN §12, Phase 0).

### 1.3 Kubernetes: test clusters only

- Before any `kubectl`, `helm`, or `kind` command, check that the current context is `kind-narrows`, or a context the human named in this session.
- Always pass `--context` explicitly.
- Never use, switch to, or modify any other context.

### 1.4 No new dependencies without updating the plan

Any new crate, including dev-dependencies and new features on existing crates, must be added to PLAN §9 in the same change and called out in your summary.

### 1.5 Don't weaken checks to get green

Never:
- delete, ignore, or `#[ignore]` tests to make them pass
- add blanket `#[allow]` attributes
- loosen lints
- skip hooks or CI steps

---

## 2. PLAN.md is the source of truth

**At the start of every task**
- Read `PLAN.md`.
- Identify the current phase (§12) and work within it. Don't build ahead into later phases.

**Keeping the plan current**
- `PLAN.md` is a living document. When implementation teaches us something (a decision changes, an assumption proves wrong, a new gotcha appears), update the relevant section in the same change.
- For each such update, add a line to a `## Revision log` section at the bottom of `PLAN.md`. Create the section if it's missing. Format: `YYYY-MM-DD · §N · what changed · why`.
- Small edits (typos, wording, source links) don't need a log entry.

**Ask before you**
- change goals or non-goals (§1)
- change phase order or exit criteria (§12)
- accept or reject an ADR (§15)

You may propose any of these in your summary.

**When code and plan disagree**
- If it isn't obvious which one is right, stop and ask. Don't silently diverge.

Section numbers refer to PLAN v0.1. If the plan is renumbered, update this file too.

---

## 3. Rust guidelines

### 3.1 Primary guide

The primary guide is **Apollo GraphQL's Rust Best Practices**, vendored as a skill at `.claude/skills/rust-best-practices/` (MIT; see `UPSTREAM.md` there).

- Before writing or reviewing Rust, read that `SKILL.md` and the chapters relevant to the task (`references/chapter_NN.md`).
- For public library APIs, also follow the [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/).

### 3.2 Project-specific rules

These override the guide wherever they conflict.

**Error handling**
- `thiserror` is approved for every crate except `narrows-ebpf` and `narrows-common`.
- `anyhow` is approved only in `narrows-tui` and `xtask`.
- `insta` is approved as a dev-dependency.
- Add each to PLAN §9 the first time it's used.

**The shim (`narrows-cni`)**
- No async runtime and no `anyhow`.
- stdout carries only the CNI result or error JSON. Diagnostics go to stderr or the log file.
- Keep it small and fast to start (PLAN §4).

**`narrows-ebpf` and `narrows-common`**
- `no_std`, with no allocation and no panicking paths.
- Loops must be bounded.
- Shared types are `#[repr(C)]`, fixed-size, and carry a version field.
- The guide's advice about `std` types doesn't apply to these crates.
- If building for the BPF target needs a nightly toolchain, pin that toolchain inside `xtask` only. Never make nightly the workspace default.

**`unsafe` code**
- Use it only where required: syscalls such as `setns`, BPF map value casts, and eBPF programs.
- Keep it in small, dedicated modules.
- Every `unsafe` block gets a `// SAFETY:` comment stating the invariant it relies on.
- Every `unsafe fn` gets a `# Safety` doc section.

**Panics**
- No `unwrap()` or `expect()` outside tests.
- In the agent, a failing reconciler returns an error and requeues. It must never take down the process, because the datapath has to keep running.

**Async code**
- No blocking calls on tokio worker threads. Use async sockets or `spawn_blocking`.
- Use bounded channels only.
- Don't share mutable state across subsystems (PLAN §5.1).

**Hot paths**
- In eBPF: no per-packet allocation, formatting, or logging.
- In userspace: avoid per-event allocation in the flow pipeline where practical.
- Measure before optimizing, and record results per PLAN §10.

**Lint exceptions**
- Use `#[expect(lint, reason = "...")]`, never `#[allow]`.

**Toolchain**
- Edition 2024, with the MSRV pinned via `rust-version` in the workspace `Cargo.toml`.

### 3.3 Workspace lints

Configure these in the root `Cargo.toml` under `[workspace.lints]`, and have every crate opt in with `lints.workspace = true`.

**Lint groups**
- `clippy::all` = deny, and `clippy::pedantic` = warn.
- Give both groups `priority = -1`, so the individual lints below can override them.

**Individual lints**
- **rustc:** `unsafe_op_in_unsafe_fn` = deny. `missing_docs` = warn, for library crates.
- **Clippy, deny:** `undocumented_unsafe_blocks`, `unwrap_used`, `expect_used`, `dbg_macro`.
- **Clippy, warn:** `todo`.

**Test exception**
- In `clippy.toml`, set `allow-unwrap-in-tests` and `allow-expect-in-tests` to true.

---

## 4. Commands

A task isn't done until all of these pass. None of them need privileges.

1. `cargo fmt --all -- --check`
2. `cargo clippy --workspace --all-targets --locked -- -D warnings`
3. `cargo test --workspace --locked`
4. `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps`

`cargo xtask ci` runs all four, in this order, stopping at the first failure — a one-shot local check before pushing. `.github/workflows/ci.yml` runs the same four commands as separate steps, so a failure is visible at a glance in the Actions UI; keep the two lists in sync by hand when this section changes, since neither includes the other.

**eBPF and packaging** go through `cargo xtask` (PLAN §11).

**Privileged and cluster tests** run only when the human asks (see §1.2 and §1.3):
- `cargo xtask test-netns`
- `cargo xtask e2e`

**If a command doesn't exist yet**, say so. Don't invent a substitute. Keep this section in sync as the build evolves.

---

## 5. Working style

**Scope**
- Make small, reviewable changes, one concern per change.
- Every behaviour change comes with tests. Name them as the guide describes (Chapter 5).

**Teaching comments**
- This is a learning project. The first time a kernel or CNI concept appears in the code, explain *why* in a doc comment, or in a note under `docs/notes/` linked to the relevant PLAN section.
- Don't narrate *what* the code does.
- Keep it short: a few lines per point, one point per comment. Mirror the density of upstream Go CNI plugins (`containernetworking/plugins`, `pkg/ns`) — terse, one non-obvious fact per comment, not a multi-paragraph essay. If an explanation needs more room than that, it belongs in `docs/notes/`, linked from the code, not inline.

**End every task with a summary** containing:
- what changed, and which files were touched
- the commands you ran and their results
- any `PLAN.md` updates
- open questions
- a suggested commit message for the human to use

---

## 6. Repo map

See PLAN §11 for the authoritative layout.

| Path | Purpose |
|---|---|
| `crates/narrows-cni` | CNI shim binary |
| `crates/narrows-agent` | Node agent (DaemonSet) |
| `crates/narrows-ebpf` | eBPF programs (Aya, `no_std`) |
| `crates/narrows-common` | Types shared by the kernel and userspace sides |
| `crates/narrows-tui` | Flow-visualization TUI |
| `xtask/` | Build, test, and packaging tasks |
| `deploy/` | Manifests and kind configs |
| `tests/`, `bench/` | Tests and benchmarks |
| `docs/adr/` | Architecture decision records |

---

## 7. Enforcement

- **Claude Code** enforces rule 1.1 with `.claude/settings.json` and `.claude/hooks/block-git-writes.sh`.
- **Other agents** rely on these instructions alone. Configure your tool's approval or sandbox settings so git and shell commands need approval.
