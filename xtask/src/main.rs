//! `xtask`: build, test, and packaging tasks for Narrows (PLAN §11).
//!
//! Run it as `cargo xtask <task>` (the alias lives in `.cargo/config.toml`).
//! The eBPF build and `e2e` arrive in the phases that need them.

use std::process::{Command, ExitCode};

const USAGE: &str = "\
usage: cargo xtask <task>

tasks:
  ci          run the checks every change must pass (AGENTS.md §4)
  test-netns  run the privileged tests/netns suite (needs CAP_NET_ADMIN and
              CAP_SYS_ADMIN, and the human's go-ahead — AGENTS.md §1.2)
  help        show this message

No other tasks are implemented yet (see PLAN.md §12).";

fn main() -> ExitCode {
    let task = std::env::args().nth(1);
    match task.as_deref() {
        Some("ci") => ci(),
        Some("test-netns") => test_netns(),
        None | Some("help" | "-h" | "--help") => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("xtask: unknown task `{other}`\n\n{USAGE}");
            ExitCode::FAILURE
        }
    }
}

/// Builds and runs `tests/netns` (`netns-tests`), the privileged suite.
///
/// This doesn't check for `CAP_NET_ADMIN`/`CAP_SYS_ADMIN` itself, and
/// doesn't manage a container or VM to run inside — that's a human decision
/// (AGENTS.md §1.2), not something to guess at or automate around. Without
/// the right capabilities, `netns-tests` reports each check it can't run
/// clearly and still exits successfully; it's `cargo xtask ci`'s job to stay
/// privilege-free, not this one's.
fn test_netns() -> ExitCode {
    match Command::new("cargo")
        .args(["run", "--package", "netns-tests", "--locked"])
        .status()
    {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        Ok(_) => ExitCode::FAILURE,
        Err(source) => {
            eprintln!("xtask: could not run `cargo`: {source}");
            ExitCode::FAILURE
        }
    }
}

/// One step of `ci`: a label to print, the `cargo` subcommand and its
/// arguments, and any extra environment variables the step needs.
struct Step {
    label: &'static str,
    args: &'static [&'static str],
    env: &'static [(&'static str, &'static str)],
}

/// The exact checks AGENTS.md §4 lists, in the same order, run the same way
/// a human would run them by hand. This function is the single source of
/// truth both the CI workflow and a local `cargo xtask ci` call go through,
/// so the two can't drift apart.
const STEPS: &[Step] = &[
    Step {
        label: "cargo fmt --all -- --check",
        args: &["fmt", "--all", "--", "--check"],
        env: &[],
    },
    Step {
        label: "cargo clippy --workspace --all-targets --locked -- -D warnings",
        args: &[
            "clippy",
            "--workspace",
            "--all-targets",
            "--locked",
            "--",
            "-D",
            "warnings",
        ],
        env: &[],
    },
    Step {
        label: "cargo test --workspace --locked",
        args: &["test", "--workspace", "--locked"],
        env: &[],
    },
    Step {
        label: "cargo doc --workspace --no-deps",
        args: &["doc", "--workspace", "--no-deps"],
        env: &[("RUSTDOCFLAGS", "-D warnings")],
    },
];

/// Runs every step in [`STEPS`], stopping at the first failure.
fn ci() -> ExitCode {
    for step in STEPS {
        println!("\n▶ {}", step.label);
        let mut command = Command::new("cargo");
        command.args(step.args);
        for (key, value) in step.env {
            command.env(key, value);
        }
        let status = match command.status() {
            Ok(status) => status,
            Err(source) => {
                eprintln!("xtask: could not run `cargo`: {source}");
                return ExitCode::FAILURE;
            }
        };
        if !status.success() {
            eprintln!("\nxtask: failed: {}", step.label);
            return ExitCode::FAILURE;
        }
    }
    println!("\n✔ all checks passed");
    ExitCode::SUCCESS
}
