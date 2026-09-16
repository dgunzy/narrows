//! The `CNI_*` environment variables.
//!
//! A CNI plugin is not a server. The runtime `exec`s a fresh process for every
//! operation and passes the per-call parameters (which container, which
//! namespace, which interface) as environment variables. The network
//! configuration comes separately, on stdin. Each verb needs a different
//! subset of the variables, so this module reads only what the verb uses.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::command::Command;
use crate::error::{CniError, ErrorCode};

/// The kernel's `IFNAMSIZ` is 16 bytes including the trailing NUL, so an
/// interface name can be at most 15 bytes. Rejecting longer names here gives
/// a clear error instead of a netlink `EINVAL` later.
const MAX_IFNAME_LEN: usize = 15;

/// The parsed `CNI_*` environment for one invocation.
///
/// Variables the verb doesn't use are never read, so they're `None` or empty
/// even when set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CniEnv {
    /// `CNI_COMMAND`.
    pub command: Command,
    /// `CNI_CONTAINERID`: the runtime's ID for the container.
    pub container_id: Option<String>,
    /// `CNI_NETNS`: a path to the container's network namespace, such as
    /// `/run/netns/<name>`.
    ///
    /// It's optional for DEL. By the time a DEL arrives the namespace may
    /// already be gone, and DEL must still clean up what it can.
    pub netns: Option<String>,
    /// `CNI_IFNAME`: the interface name to create inside the container.
    pub ifname: Option<String>,
    /// `CNI_ARGS`: extra `KEY=VALUE` pairs, in order. Kubernetes runtimes put
    /// the pod's namespace and name here, as `K8S_POD_NAMESPACE` and
    /// `K8S_POD_NAME`.
    pub args: Vec<(String, String)>,
    /// `CNI_PATH`: directories to search for delegate plugin binaries.
    pub path: Vec<PathBuf>,
}

/// Whether a verb requires a variable or merely accepts it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Need {
    Required,
    Optional,
}

/// Which variables each verb uses, from the per-operation lists in the spec.
/// `None` means the verb doesn't use the variable at all.
///
/// libcni's `skel` also requires `CNI_PATH` for ADD, DEL, CHECK, and STATUS.
/// The spec lists it as optional for those verbs, and Narrows follows the
/// spec.
const fn need(command: Command, var: &str) -> Option<Need> {
    use Command::{Add, Check, Del, Gc, Status, Version};
    use Need::{Optional, Required};

    match (command, var.as_bytes()) {
        (Add | Del | Check, b"CNI_CONTAINERID" | b"CNI_IFNAME")
        | (Add | Check, b"CNI_NETNS")
        | (Gc, b"CNI_PATH") => Some(Required),
        (Del, b"CNI_NETNS")
        | (Add | Del | Check, b"CNI_ARGS")
        | (Add | Del | Check | Status, b"CNI_PATH") => Some(Optional),
        (Version | Add | Del | Check | Status | Gc, _) => None,
    }
}

const VARS: [&str; 5] = [
    "CNI_CONTAINERID",
    "CNI_NETNS",
    "CNI_IFNAME",
    "CNI_ARGS",
    "CNI_PATH",
];

impl CniEnv {
    /// Reads and validates the environment through `getenv`.
    ///
    /// Taking a getter instead of reading `std::env` keeps this testable
    /// without mutating the process environment, which is `unsafe` in
    /// edition 2024.
    ///
    /// # Errors
    ///
    /// Returns code 4 (invalid environment variables) if `CNI_COMMAND` is
    /// missing or unknown, if a variable the verb requires is missing or
    /// empty, or if any variable it reads is not UTF-8 or badly formed.
    pub fn from_getter(getenv: impl Fn(&str) -> Option<OsString>) -> Result<Self, CniError> {
        let command: Command = match read(&getenv, "CNI_COMMAND")? {
            Some(value) => value.parse()?,
            None => return Err(missing(&["CNI_COMMAND"])),
        };

        let mut values: [Option<String>; VARS.len()] = Default::default();
        let mut absent = Vec::new();
        for (var, slot) in VARS.iter().zip(&mut values) {
            let Some(need) = need(command, var) else {
                continue;
            };
            *slot = read(&getenv, var)?;
            if slot.is_none() && need == Need::Required {
                absent.push(*var);
            }
        }
        if !absent.is_empty() {
            return Err(missing(&absent));
        }

        let [container_id, netns, ifname, args, path] = values;
        if let Some(id) = &container_id {
            validate_container_id(id)?;
        }
        if let Some(name) = &ifname {
            validate_ifname(name)?;
        }

        Ok(Self {
            command,
            container_id,
            netns,
            ifname,
            args: args
                .as_deref()
                .map(parse_args)
                .transpose()?
                .unwrap_or_default(),
            path: path.map(|p| parse_path(&p)).unwrap_or_default(),
        })
    }
}

/// Reads one variable. Unset and empty are both treated as absent, as libcni
/// does.
fn read(getenv: &impl Fn(&str) -> Option<OsString>, var: &str) -> Result<Option<String>, CniError> {
    let Some(raw) = getenv(var) else {
        return Ok(None);
    };
    let value = raw.into_string().map_err(|_| {
        CniError::new(
            ErrorCode::InvalidEnvironmentVariables,
            format!("{var} is not valid UTF-8"),
        )
    })?;
    Ok((!value.is_empty()).then_some(value))
}

fn missing(vars: &[&str]) -> CniError {
    CniError::new(
        ErrorCode::InvalidEnvironmentVariables,
        format!("required env variables [{}] missing", vars.join(",")),
    )
}

/// Checks the spec's rule for container IDs: an alphanumeric first character,
/// then only alphanumerics, `_`, `.`, or `-`.
///
/// # Errors
///
/// Returns code 4 if `id` breaks the rule.
pub fn validate_container_id(id: &str) -> Result<(), CniError> {
    let mut chars = id.chars();
    let first_ok = chars.next().is_some_and(|c| c.is_ascii_alphanumeric());
    let rest_ok = chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'));
    if first_ok && rest_ok {
        Ok(())
    } else {
        Err(CniError::new(
            ErrorCode::InvalidEnvironmentVariables,
            "invalid characters in containerID",
        )
        .with_details(id))
    }
}

/// Checks that the kernel will accept `name` as an interface name.
///
/// The spec only says a plugin must return an error if it can't use the name.
/// These rules follow libcni's `ValidateInterfaceName`, which mirrors the
/// kernel's own `dev_valid_name()`.
///
/// # Errors
///
/// Returns code 4 if the name is empty, longer than 15 bytes, `.` or `..`, or
/// contains `/`, `:`, or whitespace.
pub fn validate_ifname(name: &str) -> Result<(), CniError> {
    let problem = if name.is_empty() {
        Some("interface name is empty")
    } else if name.len() > MAX_IFNAME_LEN {
        Some("interface name is too long")
    } else if name == "." || name == ".." {
        Some("interface name is . or ..")
    } else if name
        .chars()
        .any(|c| c == '/' || c == ':' || c.is_whitespace())
    {
        Some("interface name contains / or : or whitespace characters")
    } else {
        None
    };
    match problem {
        None => Ok(()),
        Some(msg) => {
            Err(CniError::new(ErrorCode::InvalidEnvironmentVariables, msg).with_details(name))
        }
    }
}

/// Parses `CNI_ARGS`, for example `K8S_POD_NAMESPACE=default;K8S_POD_NAME=web`.
///
/// The spec calls these "alphanumeric key-value pairs", but real runtimes send
/// values containing `-` and `.`. So only the structure is enforced: each
/// `;`-separated pair has exactly one `=` and a non-empty key.
///
/// # Errors
///
/// Returns code 4 for a pair that doesn't have that structure.
pub fn parse_args(raw: &str) -> Result<Vec<(String, String)>, CniError> {
    raw.split(';')
        .map(|pair| match pair.split_once('=') {
            Some((key, value)) if !key.is_empty() && !value.contains('=') => {
                Ok((key.to_owned(), value.to_owned()))
            }
            _ => Err(CniError::new(
                ErrorCode::InvalidEnvironmentVariables,
                "invalid CNI_ARGS pair",
            )
            .with_details(pair)),
        })
        .collect()
}

/// Splits `CNI_PATH` with the platform's path-list separator (`:` on Linux),
/// dropping empty entries.
fn parse_path(raw: &str) -> Vec<PathBuf> {
    std::env::split_paths(raw)
        .filter(|p| !p.as_os_str().is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a getter over a fixed set of variables.
    fn env(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let owned: Vec<(String, OsString)> = vars
            .iter()
            .map(|(k, v)| ((*k).to_owned(), OsString::from(v)))
            .collect();
        move |key| owned.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
    }

    const ADD_ENV: &[(&str, &str)] = &[
        ("CNI_COMMAND", "ADD"),
        ("CNI_CONTAINERID", "abc123"),
        ("CNI_NETNS", "/run/netns/test"),
        ("CNI_IFNAME", "eth0"),
    ];

    mod from_getter {
        use super::*;

        #[test]
        fn parses_minimal_add() {
            let parsed = CniEnv::from_getter(env(ADD_ENV)).unwrap();

            assert_eq!(
                parsed,
                CniEnv {
                    command: Command::Add,
                    container_id: Some("abc123".into()),
                    netns: Some("/run/netns/test".into()),
                    ifname: Some("eth0".into()),
                    args: Vec::new(),
                    path: Vec::new(),
                }
            );
        }

        #[test]
        fn returns_code_4_when_command_missing() {
            let error = CniEnv::from_getter(env(&[])).unwrap_err();

            assert_eq!(error.msg(), "required env variables [CNI_COMMAND] missing");
        }

        #[test]
        fn lists_every_missing_variable_for_add() {
            let error = CniEnv::from_getter(env(&[("CNI_COMMAND", "ADD")])).unwrap_err();

            assert_eq!(
                error.msg(),
                "required env variables [CNI_CONTAINERID,CNI_NETNS,CNI_IFNAME] missing"
            );
        }

        #[test]
        fn treats_empty_value_as_missing() {
            let error = CniEnv::from_getter(env(&[
                ("CNI_COMMAND", "ADD"),
                ("CNI_CONTAINERID", "abc123"),
                ("CNI_NETNS", ""),
                ("CNI_IFNAME", "eth0"),
            ]))
            .unwrap_err();

            assert_eq!(error.msg(), "required env variables [CNI_NETNS] missing");
        }

        #[test]
        fn requires_netns_for_check() {
            let error = CniEnv::from_getter(env(&[
                ("CNI_COMMAND", "CHECK"),
                ("CNI_CONTAINERID", "abc123"),
                ("CNI_IFNAME", "eth0"),
            ]))
            .unwrap_err();

            assert_eq!(error.msg(), "required env variables [CNI_NETNS] missing");
        }

        #[test]
        fn accepts_del_without_netns() {
            let parsed = CniEnv::from_getter(env(&[
                ("CNI_COMMAND", "DEL"),
                ("CNI_CONTAINERID", "abc123"),
                ("CNI_IFNAME", "eth0"),
            ]));

            assert!(parsed.is_ok(), "DEL rejected: {parsed:?}");
        }

        #[test]
        fn accepts_version_with_only_command() {
            let parsed = CniEnv::from_getter(env(&[("CNI_COMMAND", "VERSION")]));

            assert!(parsed.is_ok(), "VERSION rejected: {parsed:?}");
        }

        #[test]
        fn ignores_variables_version_does_not_use() {
            let parsed = CniEnv::from_getter(env(&[
                ("CNI_COMMAND", "VERSION"),
                ("CNI_IFNAME", "this name is far too long"),
            ]));

            assert!(parsed.is_ok(), "VERSION rejected: {parsed:?}");
        }

        #[test]
        fn accepts_status_with_only_command() {
            let parsed = CniEnv::from_getter(env(&[("CNI_COMMAND", "STATUS")]));

            assert!(parsed.is_ok(), "STATUS rejected: {parsed:?}");
        }

        #[test]
        fn requires_path_for_gc() {
            let error = CniEnv::from_getter(env(&[("CNI_COMMAND", "GC")])).unwrap_err();

            assert_eq!(error.msg(), "required env variables [CNI_PATH] missing");
        }

        #[test]
        fn splits_path_into_directories() {
            let parsed = CniEnv::from_getter(env(&[
                ("CNI_COMMAND", "GC"),
                ("CNI_PATH", "/opt/cni/bin::/usr/libexec/cni"),
            ]))
            .unwrap();

            assert_eq!(
                parsed.path,
                [
                    PathBuf::from("/opt/cni/bin"),
                    PathBuf::from("/usr/libexec/cni")
                ]
            );
        }

        #[test]
        fn rejects_invalid_container_id() {
            let mut vars = ADD_ENV.to_vec();
            vars[1] = ("CNI_CONTAINERID", "-leading-dash");

            let error = CniEnv::from_getter(env(&vars)).unwrap_err();

            assert_eq!(error.msg(), "invalid characters in containerID");
        }

        #[test]
        fn rejects_invalid_ifname() {
            let mut vars = ADD_ENV.to_vec();
            vars[3] = ("CNI_IFNAME", "eth 0");

            let error = CniEnv::from_getter(env(&vars)).unwrap_err();

            assert_eq!(error.code(), ErrorCode::InvalidEnvironmentVariables);
        }

        #[cfg(unix)]
        #[test]
        fn returns_code_4_when_value_not_utf8() {
            use std::os::unix::ffi::OsStringExt;

            let getter = |key: &str| match key {
                "CNI_COMMAND" => Some(OsString::from("DEL")),
                "CNI_CONTAINERID" => Some(OsString::from_vec(vec![0xff, 0xfe])),
                "CNI_IFNAME" => Some(OsString::from("eth0")),
                _ => None,
            };

            let error = CniEnv::from_getter(getter).unwrap_err();

            assert_eq!(error.msg(), "CNI_CONTAINERID is not valid UTF-8");
        }
    }

    mod validate_container_id {
        use super::*;

        #[test]
        fn accepts_hex_id() {
            assert!(validate_container_id("0f3a9c").is_ok());
        }

        #[test]
        fn accepts_underscore_dot_and_dash_after_first_char() {
            assert!(validate_container_id("a_b.c-d").is_ok());
        }

        #[test]
        fn rejects_leading_underscore() {
            assert!(validate_container_id("_abc").is_err());
        }

        #[test]
        fn rejects_slash() {
            assert!(validate_container_id("abc/def").is_err());
        }

        #[test]
        fn rejects_empty() {
            assert!(validate_container_id("").is_err());
        }
    }

    mod validate_ifname {
        use super::*;

        #[test]
        fn accepts_15_bytes() {
            assert!(validate_ifname("abcdefghijklmno").is_ok());
        }

        #[test]
        fn rejects_16_bytes() {
            let error = validate_ifname("abcdefghijklmnop").unwrap_err();

            assert_eq!(error.msg(), "interface name is too long");
        }

        #[test]
        fn rejects_dot() {
            assert!(validate_ifname(".").is_err());
        }

        #[test]
        fn rejects_dot_dot() {
            assert!(validate_ifname("..").is_err());
        }

        #[test]
        fn rejects_slash() {
            assert!(validate_ifname("eth/0").is_err());
        }

        #[test]
        fn rejects_colon() {
            assert!(validate_ifname("eth0:1").is_err());
        }

        #[test]
        fn rejects_tab() {
            assert!(validate_ifname("eth\t0").is_err());
        }
    }

    mod parse_args {
        use super::*;

        #[test]
        fn parses_kubernetes_pod_args() {
            let args =
                parse_args("K8S_POD_NAMESPACE=kube-system;K8S_POD_NAME=coredns-7db6d8ff4d-x2x9z")
                    .unwrap();

            assert_eq!(
                args,
                [
                    ("K8S_POD_NAMESPACE".to_owned(), "kube-system".to_owned()),
                    (
                        "K8S_POD_NAME".to_owned(),
                        "coredns-7db6d8ff4d-x2x9z".to_owned()
                    ),
                ]
            );
        }

        #[test]
        fn accepts_empty_value() {
            let args = parse_args("IgnoreUnknown=").unwrap();

            assert_eq!(args, [("IgnoreUnknown".to_owned(), String::new())]);
        }

        #[test]
        fn rejects_pair_without_equals() {
            let error = parse_args("FOO=BAR;BAZ").unwrap_err();

            assert_eq!(error.details(), "BAZ");
        }

        #[test]
        fn rejects_pair_with_two_equals() {
            assert!(parse_args("A=B=C").is_err());
        }

        #[test]
        fn rejects_trailing_separator() {
            assert!(parse_args("FOO=BAR;").is_err());
        }
    }
}
