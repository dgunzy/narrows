//! The network configuration the runtime writes to the plugin's stdin.
//!
//! On disk, a `.conflist` in `/etc/cni/net.d` lists a chain of plugins. The
//! plugin never sees that file. For each plugin in the chain, the runtime
//! builds an "execution configuration": that plugin's own object from the
//! list, plus `cniVersion` and `name` copied from the top level, plus
//! runtime-generated keys such as `runtimeConfig` and `prevResult`.
//!
//! Only the keys the spec defines are modelled. Everything that's opaque for
//! now (`ipam`, `runtimeConfig`, `prevResult`, and so on) is kept as raw JSON
//! until a later slice gives it a type. Unknown keys are ignored, because
//! plugin-specific keys are normal in CNI configs.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::env::validate_container_id;
use crate::error::{CniError, ErrorCode};

/// A plugin's execution configuration, decoded from stdin.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetConf {
    /// The spec version the runtime selected for this call.
    #[serde(default)]
    pub cni_version: String,
    /// The network name, copied from the top-level config.
    #[serde(default)]
    pub name: String,
    /// The plugin binary's file name, as found on disk.
    #[serde(default, rename = "type")]
    pub plugin_type: String,
    /// Capabilities the plugin supports, such as `portMappings`. The runtime
    /// fills `runtimeConfig` only for capabilities listed here.
    pub capabilities: Option<Value>,
    /// Whether to set up IP masquerade on the host.
    pub ip_masq: Option<bool>,
    /// IPAM settings. Opaque until Narrows' IPAM exists.
    pub ipam: Option<Value>,
    /// DNS settings to hand back in the result.
    pub dns: Option<Dns>,
    /// Runtime-supplied values for the capabilities above.
    pub runtime_config: Option<Value>,
    /// Extra arguments supplied by the runtime in the config.
    pub args: Option<Value>,
    /// The previous plugin's result in a chain. Absent for the first ADD.
    pub prev_result: Option<Value>,
}

/// The well-known `dns` object.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Dns {
    /// Nameserver addresses, in priority order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nameservers: Vec<String>,
    /// The local domain used for short hostname lookups.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    /// Search domains, in priority order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub search: Vec<String>,
    /// Resolver options.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
}

impl NetConf {
    /// Decodes and validates a configuration from raw stdin bytes.
    ///
    /// # Errors
    ///
    /// Returns code 6 (decoding failure) if `bytes` isn't a JSON object of
    /// the expected shape. Returns code 7 (invalid network config) if
    /// `cniVersion`, `name`, or `type` is missing or malformed.
    pub fn from_slice(bytes: &[u8]) -> Result<Self, CniError> {
        let conf: Self = serde_json::from_slice(bytes).map_err(|e| {
            CniError::new(
                ErrorCode::DecodingFailure,
                "failed to decode network configuration",
            )
            .with_details(e.to_string())
        })?;
        conf.validate()?;
        Ok(conf)
    }

    fn validate(&self) -> Result<(), CniError> {
        if self.cni_version.is_empty() {
            return Err(invalid("missing cniVersion"));
        }
        if self.name.is_empty() {
            return Err(invalid("missing network name"));
        }
        // The spec gives network names the same character rules as container
        // IDs. Reuse that check, but report it as a config error, not an env one.
        if validate_container_id(&self.name).is_err() {
            return Err(
                invalid("invalid characters found in network name").with_details(&self.name)
            );
        }
        if self.plugin_type.is_empty() {
            return Err(invalid("missing type"));
        }
        // `type` becomes a file name the runtime looks up in CNI_PATH, so a path
        // separator in it would let a config escape those directories.
        if self.plugin_type.contains(['/', '\\']) {
            return Err(
                invalid("type must not contain path separators").with_details(&self.plugin_type)
            );
        }
        Ok(())
    }
}

fn invalid(msg: &str) -> CniError {
    CniError::new(ErrorCode::InvalidNetworkConfig, msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    mod from_slice {
        use super::*;

        #[test]
        fn decodes_minimal_config() {
            let conf = NetConf::from_slice(
                br#"{"cniVersion":"1.1.0","name":"narrows","type":"narrows-cni"}"#,
            )
            .unwrap();

            assert_eq!(
                (
                    conf.cni_version.as_str(),
                    conf.name.as_str(),
                    conf.plugin_type.as_str()
                ),
                ("1.1.0", "narrows", "narrows-cni")
            );
        }

        #[test]
        fn decodes_dns_fields() {
            let conf = NetConf::from_slice(
                br#"{"cniVersion":"1.1.0","name":"n","type":"t",
                    "dns":{"nameservers":["10.96.0.10"],"domain":"cluster.local",
                           "search":["svc.cluster.local"],"options":["ndots:5"]}}"#,
            )
            .unwrap();

            assert_eq!(
                conf.dns,
                Some(Dns {
                    nameservers: vec!["10.96.0.10".into()],
                    domain: Some("cluster.local".into()),
                    search: vec!["svc.cluster.local".into()],
                    options: vec!["ndots:5".into()],
                })
            );
        }

        #[test]
        fn keeps_prev_result_as_raw_json() {
            let conf = NetConf::from_slice(
                br#"{"cniVersion":"1.1.0","name":"n","type":"t","prevResult":{"ips":[]}}"#,
            )
            .unwrap();

            assert_eq!(conf.prev_result, Some(serde_json::json!({"ips": []})));
        }

        #[test]
        fn ignores_unknown_keys() {
            let conf = NetConf::from_slice(
                br#"{"cniVersion":"1.1.0","name":"n","type":"t","narrowsMode":"host-gw"}"#,
            );

            assert!(conf.is_ok(), "unknown key rejected: {conf:?}");
        }

        #[test]
        fn returns_code_6_for_malformed_json() {
            let error = NetConf::from_slice(b"{not json").unwrap_err();

            assert_eq!(error.code(), ErrorCode::DecodingFailure);
        }

        #[test]
        fn returns_code_6_for_empty_input() {
            let error = NetConf::from_slice(b"").unwrap_err();

            assert_eq!(error.code(), ErrorCode::DecodingFailure);
        }

        #[test]
        fn returns_code_6_when_field_has_wrong_type() {
            let error = NetConf::from_slice(
                br#"{"cniVersion":"1.1.0","name":"n","type":"t","ipMasq":"yes"}"#,
            )
            .unwrap_err();

            assert_eq!(error.code(), ErrorCode::DecodingFailure);
        }

        #[test]
        fn returns_code_7_when_type_missing() {
            let error = NetConf::from_slice(br#"{"cniVersion":"1.1.0","name":"n"}"#).unwrap_err();

            assert_eq!(error.msg(), "missing type");
        }

        #[test]
        fn returns_code_7_when_cni_version_missing() {
            let error = NetConf::from_slice(br#"{"name":"n","type":"t"}"#).unwrap_err();

            assert_eq!(error.msg(), "missing cniVersion");
        }

        #[test]
        fn returns_code_7_when_type_contains_slash() {
            let error =
                NetConf::from_slice(br#"{"cniVersion":"1.1.0","name":"n","type":"../bin/sh"}"#)
                    .unwrap_err();

            assert_eq!(error.code(), ErrorCode::InvalidNetworkConfig);
        }

        #[test]
        fn returns_code_7_when_name_has_invalid_characters() {
            let error =
                NetConf::from_slice(br#"{"cniVersion":"1.1.0","name":"my net","type":"t"}"#)
                    .unwrap_err();

            assert_eq!(error.msg(), "invalid characters found in network name");
        }
    }
}
