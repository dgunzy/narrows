//! CNI spec versions: parsing, comparison, and the VERSION verb's result.
//!
//! VERSION is how a runtime discovers which protocol versions a plugin speaks
//! before sending it anything else. It's answered locally, without the agent
//! (PLAN §4). That way a runtime can probe the plugin even when nothing else
//! on the node is healthy.

use std::fmt;
use std::str::FromStr;

use serde::Serialize;

/// The newest spec version Narrows implements.
pub const CURRENT_VERSION: &str = "1.1.0";

/// Every spec version Narrows accepts.
///
/// Only 1.x is listed because the result schema changed at 1.0.0, and Narrows
/// only produces 1.x results.
pub const SUPPORTED_VERSIONS: &[&str] = &["1.0.0", "1.1.0"];

/// The version that introduced the STATUS and GC verbs.
pub const STATUS_GC_MIN_VERSION: SemVer = SemVer::new(1, 1, 0);

/// A `MAJOR.MINOR.PATCH` version, as used in `cniVersion`.
///
/// The spec says `cniVersion` is Semantic Version 2.0, but no CNI version has
/// ever used pre-release or build metadata, so those are rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct SemVer {
    major: u64,
    minor: u64,
    patch: u64,
}

impl SemVer {
    /// Creates a version from its three components.
    #[must_use]
    pub const fn new(major: u64, minor: u64, patch: u64) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }
}

impl fmt::Display for SemVer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// The string wasn't a `MAJOR.MINOR.PATCH` version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseSemVerError;

impl fmt::Display for ParseSemVerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("expected a MAJOR.MINOR.PATCH version")
    }
}

impl std::error::Error for ParseSemVerError {}

impl FromStr for SemVer {
    type Err = ParseSemVerError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut parts = s.split('.').map(parse_component);
        let (Some(major), Some(minor), Some(patch), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(ParseSemVerError);
        };
        Ok(Self::new(major?, minor?, patch?))
    }
}

/// Parses one numeric component. `SemVer` 2.0 forbids leading zeros, and
/// `u64::from_str` would accept a leading `+`, so both are checked by hand.
fn parse_component(part: &str) -> Result<u64, ParseSemVerError> {
    let all_digits = !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
    let leading_zero = part.len() > 1 && part.starts_with('0');
    if !all_digits || leading_zero {
        return Err(ParseSemVerError);
    }
    part.parse().map_err(|_| ParseSemVerError)
}

/// Whether Narrows accepts `version` as a request's `cniVersion`.
#[must_use]
pub fn is_supported(version: &str) -> bool {
    SUPPORTED_VERSIONS.contains(&version)
}

/// The VERSION verb's result.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionResult<'a> {
    cni_version: &'a str,
    supported_versions: &'static [&'static str],
}

impl<'a> VersionResult<'a> {
    /// Builds the result, echoing the caller's `cniVersion` as the spec
    /// requires.
    #[must_use]
    pub const fn new(cni_version: &'a str) -> Self {
        Self {
            cni_version,
            supported_versions: SUPPORTED_VERSIONS,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod semver_from_str {
        use super::*;

        #[test]
        fn parses_three_components() {
            assert_eq!("1.10.2".parse(), Ok(SemVer::new(1, 10, 2)));
        }

        #[test]
        fn rejects_two_components() {
            assert_eq!("1.1".parse::<SemVer>(), Err(ParseSemVerError));
        }

        #[test]
        fn rejects_four_components() {
            assert_eq!("1.1.0.0".parse::<SemVer>(), Err(ParseSemVerError));
        }

        #[test]
        fn rejects_leading_zero() {
            assert_eq!("1.01.0".parse::<SemVer>(), Err(ParseSemVerError));
        }

        #[test]
        fn rejects_plus_sign() {
            assert_eq!("+1.1.0".parse::<SemVer>(), Err(ParseSemVerError));
        }

        #[test]
        fn rejects_pre_release_suffix() {
            assert_eq!("1.1.0-rc1".parse::<SemVer>(), Err(ParseSemVerError));
        }

        #[test]
        fn rejects_empty_component() {
            assert_eq!("1..0".parse::<SemVer>(), Err(ParseSemVerError));
        }
    }

    mod semver_ord {
        use super::*;

        #[test]
        fn compares_numerically_not_lexically() {
            assert!(SemVer::new(1, 10, 0) > SemVer::new(1, 9, 0));
        }
    }

    mod is_supported {
        use super::*;

        #[test]
        fn accepts_1_1_0() {
            assert!(is_supported("1.1.0"));
        }

        #[test]
        fn accepts_1_0_0() {
            assert!(is_supported("1.0.0"));
        }

        #[test]
        fn rejects_pre_1_0_versions() {
            assert!(!is_supported("0.4.0"));
        }
    }

    mod version_result {
        use super::*;

        #[test]
        fn serializes_spec_field_names() {
            let json = serde_json::to_value(VersionResult::new("1.0.0")).unwrap();

            assert_eq!(
                json,
                serde_json::json!({
                    "cniVersion": "1.0.0",
                    "supportedVersions": ["1.0.0", "1.1.0"],
                })
            );
        }
    }
}
