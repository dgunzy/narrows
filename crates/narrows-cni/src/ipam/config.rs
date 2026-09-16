//! The `ipam` section of the network configuration.
//!
//! In Phase 1 the fat plugin has no Kubernetes client, so it can't read
//! `Node.spec.podCIDRs`. Each node's conflist carries its pool instead:
//!
//! ```json
//! "ipam": { "subnet": "10.244.1.0/24" }
//! ```
//!
//! In Phase 2 the agent takes the pool from the Node watch (PLAN §5.3).

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::cidr::Ipv4Cidr;
use crate::config::NetConf;
use crate::error::{CniError, ErrorCode};

/// Narrows' IPAM settings.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct IpamConfig {
    /// The pool to allocate pod addresses from.
    pub subnet: Ipv4Cidr,
}

impl IpamConfig {
    /// Extracts and decodes the `ipam` section of `conf`.
    ///
    /// # Errors
    ///
    /// Returns code 7 (invalid network config) if `ipam` is missing, lacks
    /// `subnet`, or `subnet` isn't a valid CIDR.
    pub fn from_net_conf(conf: &NetConf) -> Result<Self, CniError> {
        let section = conf
            .ipam
            .as_ref()
            .ok_or_else(|| CniError::new(ErrorCode::InvalidNetworkConfig, "missing ipam"))?;
        Self::deserialize(section).map_err(|e| {
            CniError::new(
                ErrorCode::InvalidNetworkConfig,
                "invalid ipam configuration",
            )
            .with_details(e.to_string())
        })
    }
}

impl Serialize for Ipv4Cidr {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Ipv4Cidr {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse()
            .map_err(|e| serde::de::Error::custom(format!("{text:?}: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conf(json: &str) -> NetConf {
        NetConf::from_slice(json.as_bytes()).unwrap()
    }

    mod from_net_conf {
        use super::*;

        #[test]
        fn decodes_subnet() {
            let conf = conf(
                r#"{"cniVersion":"1.1.0","name":"n","type":"t","ipam":{"subnet":"10.244.1.0/24"}}"#,
            );

            assert_eq!(
                IpamConfig::from_net_conf(&conf).unwrap().subnet,
                "10.244.1.0/24".parse().unwrap()
            );
        }

        #[test]
        fn returns_code_7_when_ipam_missing() {
            let conf = conf(r#"{"cniVersion":"1.1.0","name":"n","type":"t"}"#);

            assert_eq!(
                IpamConfig::from_net_conf(&conf).unwrap_err().msg(),
                "missing ipam"
            );
        }

        #[test]
        fn returns_code_7_when_subnet_invalid() {
            let conf = conf(
                r#"{"cniVersion":"1.1.0","name":"n","type":"t","ipam":{"subnet":"10.244.1.7/24"}}"#,
            );

            assert_eq!(
                IpamConfig::from_net_conf(&conf).unwrap_err().code(),
                ErrorCode::InvalidNetworkConfig
            );
        }
    }
}
