# Development setup

How to build, check, and run a lab for Narrows. See PLAN §12 (Phase 0) for why the lab looks like this.

## Rust toolchain

- Use any recent **stable** Rust. The workspace's `rust-version = "1.96"` is the minimum supported version, and Cargo refuses anything older.
- The toolchain isn't pinned locally, on purpose. CI pins a specific version (`.github/workflows/ci.yml`), so reproducibility lives there instead (PLAN §12, Phase 0).
- Nightly is never the workspace default. If the eBPF build needs nightly, `xtask` will pin it for that build only (AGENTS §3.2).

## Checks

A change isn't done until all of these pass (AGENTS §4). None need privileges.

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
```

`--locked` fails if `Cargo.lock` is out of date. After changing dependencies, run `cargo check --workspace` once to update it.

Or run all four at once: `cargo xtask ci`. It stops at the first failure, printing which step failed — a one-shot check before pushing.

## Trying the shim by hand

The shim is safe to run on a dev machine. So far it only parses and answers, and touches nothing on the host.

```sh
cargo build -p narrows-cni
echo '{"cniVersion":"1.1.0"}' | CNI_COMMAND=VERSION target/debug/narrows-cni; echo "exit=$?"
```

## kind lab

**Prerequisites:** Docker (Docker Desktop on macOS), `kind` **v0.30.0** (the node image in the config is pinned for that version), and `kubectl`.

A human runs these; agents don't (AGENTS §1.3):

```sh
kind create cluster --config deploy/kind/narrows.yaml
kubectl --context kind-narrows get nodes   # NotReady until a CNI is installed
kind delete cluster --name narrows
```

What the config sets up, and why:

- **No default CNI.** kindnet is disabled so Narrows replaces it. Nodes report `NotReady` and CoreDNS stays `Pending` until a CNI config lands in `/etc/cni/net.d`.
- **kube-proxy in nftables mode.** Service VIPs stay kube-proxy's job until Phase 7 (ADR-6).
- **One control plane and two workers.** Multi-node from day one, so cross-node paths exist by Phase 3.

### The kernel you're actually testing

kind nodes are containers, so every node shares **one kernel**. On macOS, that's the Linux VM Docker Desktop runs, not macOS. Check it against the PLAN §2.2 baseline (6.8+ recommended; tcx needs 6.6, netkit 6.7):

```sh
docker run --rm alpine uname -r
```

Because the kernel is shared, eBPF pin paths, program names, and global sysctls collide across "nodes". Namespace them by node name from the start (PLAN §12, §14.4).

## Safety rules

- **Host networking and the kernel** (`ip`, `nft`, `sysctl`, netns, eBPF loading, `sudo`) only happen inside the kind cluster or a VM, never on the dev host (AGENTS §1.2).
- **Kubernetes commands** always pass `--context kind-narrows` (AGENTS §1.3).
