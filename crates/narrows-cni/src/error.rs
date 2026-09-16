//! CNI error codes and the error result written to stdout.
//!
//! The runtime tells a failed call apart from a successful one using two
//! signals. The exit code only says *that* the plugin failed. The error
//! result JSON on stdout says *why*, with a numeric `code` the runtime can act
//! on. For example, code 11 ("try again later") makes a runtime retry rather
//! than give up on the pod.
//!
//! Codes 0–99 belong to the spec, and 100 and up are ours (PLAN §4).

use std::fmt;

use serde::Serialize;

/// A CNI error code.
///
/// Values 1–11 and 50–51 are the well-known codes from the CNI spec. libcni
/// also defines 8 (`ErrInvalidNetNS`) and 999 (`ErrInternal`), but they aren't
/// in the spec's table, so Narrows doesn't use them. Narrows-specific codes
/// start at 100.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum ErrorCode {
    /// 1: the requested `cniVersion` isn't one this plugin supports.
    IncompatibleCniVersion = 1,
    /// 2: the network configuration uses a field the plugin doesn't support.
    UnsupportedField = 2,
    /// 3: the container is unknown or doesn't exist.
    UnknownContainer = 3,
    /// 4: a required `CNI_*` environment variable is missing or invalid.
    InvalidEnvironmentVariables = 4,
    /// 5: reading stdin or writing stdout failed.
    IoFailure = 5,
    /// 6: stdin couldn't be decoded as JSON of the expected shape.
    DecodingFailure = 6,
    /// 7: the configuration decoded, but its values are invalid.
    InvalidNetworkConfig = 7,
    /// 11: a transient condition; the runtime should retry later.
    TryAgainLater = 11,
    /// 50: STATUS only. The plugin can't service ADD requests.
    PluginNotAvailable = 50,
    /// 51: STATUS only. The plugin can't service ADD, and existing containers
    /// may have limited connectivity.
    LimitedConnectivity = 51,
    /// 100: Narrows recognizes the verb but doesn't implement it yet.
    NotImplemented = 100,
    /// 101: every usable address in the node's pool is leased.
    IpamExhausted = 101,
    /// 102: the persisted IPAM state file couldn't be understood.
    IpamStateCorrupt = 102,
}

impl ErrorCode {
    /// The numeric code as it appears in the error result JSON.
    #[must_use]
    pub const fn as_u32(self) -> u32 {
        self as u32
    }
}

/// A failed CNI operation: a code, a short message, and optional details.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CniError {
    code: ErrorCode,
    msg: String,
    details: String,
}

impl CniError {
    /// Creates an error with no details.
    pub fn new(code: ErrorCode, msg: impl Into<String>) -> Self {
        Self {
            code,
            msg: msg.into(),
            details: String::new(),
        }
    }

    /// Attaches a longer description of the error.
    #[must_use]
    pub fn with_details(mut self, details: impl Into<String>) -> Self {
        self.details = details.into();
        self
    }

    /// The error's code.
    #[must_use]
    pub const fn code(&self) -> ErrorCode {
        self.code
    }

    /// The short message.
    #[must_use]
    pub fn msg(&self) -> &str {
        &self.msg
    }

    /// The longer description, empty if there is none.
    #[must_use]
    pub fn details(&self) -> &str {
        &self.details
    }

    /// The spec's error result structure, tagged with `cni_version`.
    #[must_use]
    pub fn to_result<'a>(&'a self, cni_version: &'a str) -> ErrorResult<'a> {
        ErrorResult {
            cni_version,
            code: self.code.as_u32(),
            msg: &self.msg,
            details: &self.details,
        }
    }
}

impl fmt::Display for CniError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (code {})", self.msg, self.code.as_u32())?;
        if !self.details.is_empty() {
            write!(f, ": {}", self.details)?;
        }
        Ok(())
    }
}

impl std::error::Error for CniError {}

/// The JSON error result the spec requires on stdout when a plugin fails.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorResult<'a> {
    cni_version: &'a str,
    code: u32,
    msg: &'a str,
    // libcni tags this `omitempty`, so runtimes accept the key being absent.
    #[serde(skip_serializing_if = "str::is_empty")]
    details: &'a str,
}

#[cfg(test)]
mod tests {
    use super::*;

    mod to_result {
        use super::*;

        #[test]
        fn serializes_spec_field_names() {
            let error = CniError::new(ErrorCode::InvalidNetworkConfig, "bad config")
                .with_details("missing type");

            let json = serde_json::to_value(error.to_result("1.1.0")).unwrap();

            assert_eq!(
                json,
                serde_json::json!({
                    "cniVersion": "1.1.0",
                    "code": 7,
                    "msg": "bad config",
                    "details": "missing type",
                })
            );
        }

        #[test]
        fn omits_details_when_empty() {
            let error = CniError::new(ErrorCode::IoFailure, "read failed");

            let json = serde_json::to_value(error.to_result("1.1.0")).unwrap();

            assert!(json.get("details").is_none(), "unexpected details: {json}");
        }
    }

    mod error_code {
        use super::*;

        #[test]
        fn not_implemented_is_in_plugin_specific_range() {
            assert_eq!(ErrorCode::NotImplemented.as_u32(), 100);
        }
    }

    mod display {
        use super::*;

        #[test]
        fn includes_code_and_details() {
            let error = CniError::new(ErrorCode::DecodingFailure, "bad json").with_details("eof");

            assert_eq!(error.to_string(), "bad json (code 6): eof");
        }
    }
}
