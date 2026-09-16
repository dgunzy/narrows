//! Runs the real `narrows-cni` binary the way a runtime does: a clean
//! environment, config on stdin, and the result read from stdout plus the exit
//! status. These tests don't need privileges and don't touch host networking.

use std::io::Write;
use std::process::{Command, Output, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_narrows-cni");

/// Execs the plugin with only `vars` in its environment.
///
/// Returns `Result` so the `unwrap`s stay inside the `#[test]` functions,
/// where `clippy.toml` allows them.
fn exec(vars: &[(&str, &str)], stdin: &str) -> std::io::Result<Output> {
    let mut child = Command::new(BIN)
        .env_clear()
        .envs(vars.iter().copied())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    if let Some(mut pipe) = child.stdin.take() {
        pipe.write_all(stdin.as_bytes())?;
    }
    child.wait_with_output()
}

/// Parses stdout as exactly one JSON document, as a runtime would.
fn stdout_json(output: &Output) -> serde_json::Result<serde_json::Value> {
    serde_json::from_slice(&output.stdout)
}

const ADD_ENV: &[(&str, &str)] = &[
    ("CNI_COMMAND", "ADD"),
    ("CNI_CONTAINERID", "abc123"),
    ("CNI_NETNS", "/run/netns/test"),
    ("CNI_IFNAME", "eth0"),
    ("CNI_PATH", "/opt/cni/bin"),
];

mod version {
    use super::*;

    #[test]
    fn exits_zero() {
        let output = exec(&[("CNI_COMMAND", "VERSION")], r#"{"cniVersion":"1.1.0"}"#).unwrap();

        assert!(output.status.success(), "status: {:?}", output.status);
    }

    #[test]
    fn prints_version_result() {
        let output = exec(&[("CNI_COMMAND", "VERSION")], r#"{"cniVersion":"1.1.0"}"#).unwrap();

        assert_eq!(
            stdout_json(&output).unwrap(),
            serde_json::json!({"cniVersion": "1.1.0", "supportedVersions": ["1.0.0", "1.1.0"]})
        );
    }

    #[test]
    fn answers_with_empty_stdin() {
        let output = exec(&[("CNI_COMMAND", "VERSION")], "").unwrap();

        assert_eq!(stdout_json(&output).unwrap()["cniVersion"], "1.1.0");
    }
}

mod errors {
    use super::*;

    #[test]
    fn unknown_command_exits_one() {
        let output = exec(&[("CNI_COMMAND", "FROB")], "").unwrap();

        assert_eq!(output.status.code(), Some(1));
    }

    #[test]
    fn unknown_command_prints_code_4() {
        let output = exec(&[("CNI_COMMAND", "FROB")], "").unwrap();

        assert_eq!(stdout_json(&output).unwrap()["code"], 4);
    }

    #[test]
    fn add_with_valid_request_prints_not_implemented() {
        let output = exec(
            ADD_ENV,
            r#"{"cniVersion":"1.1.0","name":"narrows","type":"narrows-cni"}"#,
        )
        .unwrap();

        assert_eq!(stdout_json(&output).unwrap()["code"], 100);
    }

    #[test]
    fn add_with_unsupported_version_prints_code_1() {
        let output = exec(
            ADD_ENV,
            r#"{"cniVersion":"0.3.1","name":"narrows","type":"narrows-cni"}"#,
        )
        .unwrap();

        assert_eq!(stdout_json(&output).unwrap()["code"], 1);
    }

    #[test]
    fn diagnostics_go_to_stderr() {
        let output = exec(&[("CNI_COMMAND", "FROB")], "").unwrap();

        assert!(!output.stderr.is_empty(), "expected a diagnostic on stderr");
    }
}
