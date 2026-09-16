//! The `narrows-cni` executable. All logic lives in the library, so it can be
//! tested without spawning a process.

use std::process::ExitCode;

fn main() -> ExitCode {
    // A closure, not `std::env::var_os` directly: `var_os` is generic over its
    // key type, so as a function item it only implements `Fn` for one specific
    // `&str` lifetime, and `run` needs one that works for every lifetime.
    narrows_cni::run(
        |key| std::env::var_os(key),
        std::io::stdin().lock(),
        std::io::stdout().lock(),
    )
}
