//! `narrows-cni`: the CNI plugin binary (PLAN §4).
//!
//! # The exec contract
//!
//! A container runtime (containerd or CRI-O) doesn't link against a CNI
//! plugin or talk to it over a socket. It `exec`s the binary once per
//! operation, and everything crosses the process boundary through four
//! channels (PLAN §2.1):
//!
//! - **Environment variables** carry the per-call parameters: the verb, the
//!   container, the netns path, and the interface name ([`env`](mod@env)).
//! - **stdin** carries the network configuration as JSON ([`config`]).
//! - **stdout** carries exactly one JSON document: the result, or an error
//!   result ([`error`]). The runtime parses the whole stream, so a stray log
//!   line on stdout corrupts the response. That's why every diagnostic goes
//!   to stderr.
//! - **The exit code** says whether the call succeeded. Zero means success;
//!   anything else means the stdout JSON is an error.
//!
//! This slice implements the parsing, VERSION, and error reporting. Every
//! other verb is fully validated and then answered with code 100 (not
//! implemented).

pub mod command;
pub mod config;
pub mod env;
pub mod error;
pub mod ipam;
pub mod netlink;
pub mod netns;
pub mod result;
pub mod version;

use std::ffi::OsString;
use std::io::{Read, Write};
use std::process::ExitCode;

use serde::{Deserialize, Serialize};

use crate::command::Command;
use crate::config::NetConf;
use crate::env::CniEnv;
use crate::error::{CniError, ErrorCode};
use crate::version::{CURRENT_VERSION, STATUS_GC_MIN_VERSION, SemVer, VersionResult};

/// The most stdin will be read. Real configs are a few kilobytes, and the cap
/// stops a broken runtime from making the plugin buffer without limit.
pub const MAX_STDIN_BYTES: u64 = 1024 * 1024;

/// Runs one CNI invocation and returns the process exit code.
///
/// The process environment, stdin, and stdout are passed in so the whole
/// contract can be tested without spawning a process.
pub fn run(
    getenv: impl Fn(&str) -> Option<OsString>,
    stdin: impl Read,
    mut stdout: impl Write,
) -> ExitCode {
    let written = match dispatch(getenv, stdin) {
        Ok(version) => {
            let reply = VersionResult::new(&version);
            write_json(&mut stdout, &reply).map(|()| ExitCode::SUCCESS)
        }
        Err(failure) => {
            eprintln!("narrows-cni: {}", failure.error);
            let reply = failure.error.to_result(&failure.cni_version);
            write_json(&mut stdout, &reply).map(|()| ExitCode::FAILURE)
        }
    };
    written.unwrap_or_else(|e| {
        eprintln!("narrows-cni: failed to write result to stdout: {e}");
        ExitCode::FAILURE
    })
}

/// An error, plus the `cniVersion` to tag the error result with.
struct Failure {
    error: CniError,
    cni_version: String,
}

impl From<CniError> for Failure {
    fn from(error: CniError) -> Self {
        Self {
            error,
            cni_version: CURRENT_VERSION.to_owned(),
        }
    }
}

/// Handles the invocation. On success, returns the `cniVersion` for the
/// VERSION result, because VERSION is the only verb that can succeed so far.
fn dispatch(
    getenv: impl Fn(&str) -> Option<OsString>,
    stdin: impl Read,
) -> Result<String, Failure> {
    let env = CniEnv::from_getter(getenv)?;
    let input = read_stdin(stdin);

    if env.command == Command::Version {
        return Ok(version_to_echo(input.as_deref().unwrap_or_default()));
    }

    let conf = NetConf::from_slice(&input?)?;
    check_version(env.command, &conf.cni_version)?;

    // From here the request's version is known to be supported, so errors
    // are reported in the version the runtime asked for.
    Err(Failure {
        error: CniError::new(
            ErrorCode::NotImplemented,
            format!("{} is not implemented yet", env.command),
        ),
        cni_version: conf.cni_version,
    })
}

fn read_stdin(stdin: impl Read) -> Result<Vec<u8>, CniError> {
    let mut buf = Vec::new();
    stdin
        .take(MAX_STDIN_BYTES + 1)
        .read_to_end(&mut buf)
        .map_err(|e| {
            CniError::new(ErrorCode::IoFailure, "failed to read stdin").with_details(e.to_string())
        })?;
    if buf.len() as u64 > MAX_STDIN_BYTES {
        return Err(CniError::new(
            ErrorCode::DecodingFailure,
            format!("network configuration is larger than {MAX_STDIN_BYTES} bytes"),
        ));
    }
    Ok(buf)
}

/// Picks the `cniVersion` for the VERSION result.
///
/// The spec says to echo the `cniVersion` from stdin. libcni's `skel` ignores
/// stdin for VERSION, and people run `CNI_COMMAND=VERSION narrows-cni` by hand
/// with no input. So if the input is missing or not a valid version, this
/// falls back to [`CURRENT_VERSION`] instead of failing the one verb meant for
/// discovery.
fn version_to_echo(input: &[u8]) -> String {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct VersionRequest {
        cni_version: String,
    }

    serde_json::from_slice::<VersionRequest>(input)
        .ok()
        .map(|request| request.cni_version)
        .filter(|v| v.parse::<SemVer>().is_ok())
        .unwrap_or_else(|| CURRENT_VERSION.to_owned())
}

fn check_version(command: Command, requested: &str) -> Result<(), CniError> {
    if !version::is_supported(requested) {
        return Err(CniError::new(
            ErrorCode::IncompatibleCniVersion,
            format!(
                "incompatible CNI versions; config is {requested:?}, plugin supports {:?}",
                version::SUPPORTED_VERSIONS
            ),
        ));
    }
    // STATUS and GC didn't exist before 1.1.0, so an older config can't ask
    // for them.
    let introduced_later = matches!(command, Command::Status | Command::Gc);
    if introduced_later
        && requested
            .parse::<SemVer>()
            .is_ok_and(|v| v < STATUS_GC_MIN_VERSION)
    {
        return Err(CniError::new(
            ErrorCode::IncompatibleCniVersion,
            format!("config version {requested} does not allow {command}"),
        ));
    }
    Ok(())
}

fn write_json(stdout: &mut impl Write, value: &impl Serialize) -> std::io::Result<()> {
    serde_json::to_writer(&mut *stdout, value).map_err(std::io::Error::other)?;
    stdout.write_all(b"\n")?;
    stdout.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADD_CONFIG: &str = r#"{"cniVersion":"1.1.0","name":"narrows","type":"narrows-cni"}"#;

    fn env(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let owned: Vec<(String, OsString)> = vars
            .iter()
            .map(|(k, v)| ((*k).to_owned(), OsString::from(v)))
            .collect();
        move |key| owned.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
    }

    /// Runs an invocation and returns the exit code and stdout parsed as JSON.
    fn invoke(vars: &[(&str, &str)], stdin: &str) -> (ExitCode, serde_json::Value) {
        let mut stdout = Vec::new();
        let code = run(env(vars), stdin.as_bytes(), &mut stdout);
        let json = serde_json::from_slice(&stdout).unwrap();
        (code, json)
    }

    fn add_env() -> Vec<(&'static str, &'static str)> {
        vec![
            ("CNI_COMMAND", "ADD"),
            ("CNI_CONTAINERID", "abc123"),
            ("CNI_NETNS", "/run/netns/test"),
            ("CNI_IFNAME", "eth0"),
        ]
    }

    mod run {
        use super::*;

        #[test]
        fn version_succeeds() {
            let (code, _) = invoke(&[("CNI_COMMAND", "VERSION")], r#"{"cniVersion":"1.0.0"}"#);

            assert_eq!(code, ExitCode::SUCCESS);
        }

        #[test]
        fn version_echoes_requested_cni_version() {
            let (_, json) = invoke(&[("CNI_COMMAND", "VERSION")], r#"{"cniVersion":"1.0.0"}"#);

            assert_eq!(json["cniVersion"], "1.0.0");
        }

        #[test]
        fn version_echoes_unsupported_cni_version() {
            let (_, json) = invoke(&[("CNI_COMMAND", "VERSION")], r#"{"cniVersion":"0.4.0"}"#);

            assert_eq!(json["cniVersion"], "0.4.0");
        }

        #[test]
        fn version_falls_back_to_current_when_stdin_empty() {
            let (_, json) = invoke(&[("CNI_COMMAND", "VERSION")], "");

            assert_eq!(json["cniVersion"], CURRENT_VERSION);
        }

        #[test]
        fn version_falls_back_to_current_when_version_malformed() {
            let (_, json) = invoke(&[("CNI_COMMAND", "VERSION")], r#"{"cniVersion":"latest"}"#);

            assert_eq!(json["cniVersion"], CURRENT_VERSION);
        }

        #[test]
        fn missing_command_fails() {
            let (code, _) = invoke(&[], "");

            assert_eq!(code, ExitCode::FAILURE);
        }

        #[test]
        fn missing_command_reports_code_4() {
            let (_, json) = invoke(&[], "");

            assert_eq!(json["code"], 4);
        }

        #[test]
        fn add_with_valid_request_reports_not_implemented() {
            let (_, json) = invoke(&add_env(), ADD_CONFIG);

            assert_eq!(json["code"], 100);
        }

        #[test]
        fn add_with_bad_json_reports_code_6() {
            let (_, json) = invoke(&add_env(), "{");

            assert_eq!(json["code"], 6);
        }

        #[test]
        fn add_with_unsupported_version_reports_code_1() {
            let (_, json) = invoke(
                &add_env(),
                r#"{"cniVersion":"0.4.0","name":"n","type":"t"}"#,
            );

            assert_eq!(json["code"], 1);
        }

        #[test]
        fn error_is_tagged_with_requested_version_once_known() {
            let (_, json) = invoke(
                &add_env(),
                r#"{"cniVersion":"1.0.0","name":"n","type":"t"}"#,
            );

            assert_eq!(json["cniVersion"], "1.0.0");
        }

        #[test]
        fn status_with_1_0_0_config_reports_code_1() {
            let (_, json) = invoke(
                &[("CNI_COMMAND", "STATUS")],
                r#"{"cniVersion":"1.0.0","name":"n","type":"t"}"#,
            );

            assert_eq!(json["code"], 1);
        }

        #[test]
        fn gc_with_1_1_0_config_reports_not_implemented() {
            let (_, json) = invoke(
                &[("CNI_COMMAND", "GC"), ("CNI_PATH", "/opt/cni/bin")],
                r#"{"cniVersion":"1.1.0","name":"n","type":"t"}"#,
            );

            assert_eq!(json["code"], 100);
        }

        #[test]
        fn oversized_stdin_reports_code_6() {
            let big = " ".repeat(usize::try_from(MAX_STDIN_BYTES).unwrap() + 1);

            let (_, json) = invoke(&add_env(), &big);

            assert_eq!(json["code"], 6);
        }
    }
}
