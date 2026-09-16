//! The `CNI_COMMAND` verb.

use std::fmt;
use std::str::FromStr;

use crate::error::{CniError, ErrorCode};

/// A CNI operation, taken from `CNI_COMMAND`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// Attach a container to the network.
    Add,
    /// Detach a container. Must succeed for attachments the plugin has
    /// already forgotten, because libcni synthesizes DELs for stale cached
    /// attachments (PLAN §2.1).
    Del,
    /// Check that an existing attachment is still as it was created.
    Check,
    /// Report whether the plugin can accept ADDs (new in spec 1.1).
    Status,
    /// Release everything not in the runtime's list of valid attachments
    /// (new in spec 1.1).
    Gc,
    /// Report the spec versions the plugin supports.
    Version,
}

impl Command {
    /// The verb exactly as it appears in `CNI_COMMAND`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Add => "ADD",
            Self::Del => "DEL",
            Self::Check => "CHECK",
            Self::Status => "STATUS",
            Self::Gc => "GC",
            Self::Version => "VERSION",
        }
    }
}

impl fmt::Display for Command {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Command {
    type Err = CniError;

    /// Parses a verb. Matching is case-sensitive, like libcni's.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "ADD" => Ok(Self::Add),
            "DEL" => Ok(Self::Del),
            "CHECK" => Ok(Self::Check),
            "STATUS" => Ok(Self::Status),
            "GC" => Ok(Self::Gc),
            "VERSION" => Ok(Self::Version),
            other => Err(CniError::new(
                ErrorCode::InvalidEnvironmentVariables,
                format!("unknown CNI_COMMAND: {other}"),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod from_str {
        use super::*;

        #[test]
        fn parses_every_verb_from_its_own_name() {
            let all = [
                Command::Add,
                Command::Del,
                Command::Check,
                Command::Status,
                Command::Gc,
                Command::Version,
            ];

            let round_tripped: Vec<Command> =
                all.iter().map(|c| c.as_str().parse().unwrap()).collect();

            assert_eq!(round_tripped, all);
        }

        #[test]
        fn rejects_lowercase_verb_with_code_4() {
            let error = "add".parse::<Command>().unwrap_err();

            assert_eq!(error.code(), ErrorCode::InvalidEnvironmentVariables);
        }

        #[test]
        fn rejects_unknown_verb_with_code_4() {
            let error = "RESTART".parse::<Command>().unwrap_err();

            assert_eq!(error.code(), ErrorCode::InvalidEnvironmentVariables);
        }
    }
}
