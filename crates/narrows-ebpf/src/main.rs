//! `narrows-ebpf`: kernel-side eBPF programs (PLAN §6.4), built with Aya.
//!
//! Stub only. For the BPF target (`target_arch = "bpf"`) the crate is
//! `no_std` and `no_main`: eBPF programs have no allocator and no OS, and the
//! kernel calls each program at its attach point instead of calling a `main`.
//! On the host it compiles to an empty binary, so `cargo clippy --workspace`
//! and `cargo test --workspace` keep working without a BPF toolchain. The real
//! build goes through `cargo xtask` (Phase 4).

#![cfg_attr(target_arch = "bpf", no_std)]
#![cfg_attr(target_arch = "bpf", no_main)]

/// Required by `no_std`, but never meant to run.
///
/// eBPF can't unwind, so the `bpfel-unknown-none` target already uses
/// `panic = "abort"`, and nothing needs setting in the workspace profiles.
/// Those profiles also cover the agent, where unwinding lets a panicking
/// tokio task fail alone instead of taking down the whole process. Narrows
/// code in this crate must have no panicking paths (AGENTS §3.2). The verifier
/// would reject a reachable infinite loop anyway.
#[cfg(target_arch = "bpf")]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo<'_>) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

#[cfg(not(target_arch = "bpf"))]
fn main() {}
