//! `narrows-common`: types shared by the kernel and userspace sides.
//!
//! Stub only. It will hold `#[repr(C)]`, fixed-size, versioned POD structs:
//! map keys and values, event records, and drop reasons (PLAN §3, §8.2).
//! It's `no_std` so the eBPF crate can depend on it.

#![no_std]
