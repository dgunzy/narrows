//! The ADD success result: the JSON a plugin returns once a container is
//! attached to the network.
//!
//! DEL, CHECK, GC, and STATUS have no result payload on success; only ADD
//! does. The shape here follows the spec's 1.x result schema exactly, field
//! for field, because a chained plugin or the runtime itself may parse it.
//!
//! This module isn't wired into ADD's dispatch yet. Its fields describe
//! real interfaces, addresses, and routes, and Narrows doesn't create any of
//! those until the netlink slice lands (PLAN §12, Phase 1). Claiming success
//! before then would tell the runtime a pod has networking it doesn't have.

use std::net::Ipv4Addr;

use serde::Serialize;

use crate::config::Dns;

/// The link-local gateway every pod's default route points at (PLAN §6.1).
///
/// It's outside any pod CIDR, the way Calico's `169.254.1.1` is: a single
/// fixed address the host answers ARP for on every pod-facing interface,
/// rather than a per-pod or per-node value that would have to be tracked.
pub const GATEWAY: Ipv4Addr = Ipv4Addr::new(169, 254, 1, 1);

use crate::ipam::Ipv4Cidr;

/// The ADD success result (spec "Success").
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AddResult {
    /// The version this result is shaped for; echoes the request's.
    pub cni_version: String,
    /// Every interface the attachment created, container and host side.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub interfaces: Vec<Interface>,
    /// Addresses assigned by this attachment.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ips: Vec<IpConfig>,
    /// Routes this attachment installed.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub routes: Vec<Route>,
    /// DNS configuration for the attachment, if this plugin provides one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dns: Option<Dns>,
}

impl AddResult {
    /// Builds the result for Narrows' routed `/32` pod model (PLAN §6.1):
    /// one container-side interface, its single address, and a default
    /// route to the fixed link-local [`GATEWAY`].
    #[must_use]
    pub fn single_interface(
        cni_version: impl Into<String>,
        ifname: impl Into<String>,
        sandbox: impl Into<String>,
        address: Ipv4Addr,
        dns: Option<Dns>,
    ) -> Self {
        Self {
            cni_version: cni_version.into(),
            interfaces: vec![Interface {
                name: ifname.into(),
                mac: None,
                mtu: None,
                sandbox: sandbox.into(),
            }],
            ips: vec![IpConfig::new(address, 32, Some(GATEWAY), Some(0))],
            routes: vec![Route::new(Ipv4Cidr::UNSPECIFIED, Some(GATEWAY), None)],
            dns,
        }
    }
}

/// One interface the attachment created, container-side or host-side.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Interface {
    /// The interface's name.
    pub name: String,
    /// The hardware (MAC) address, once known. Not set until the interface
    /// actually exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mac: Option<String>,
    /// The interface's MTU, once known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mtu: Option<u32>,
    /// A reference to the isolation domain the interface lives in (typically
    /// the netns path), or empty for a host-side interface.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub sandbox: String,
}

/// One address this attachment assigned.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IpConfig {
    /// `ADDRESS/PREFIX_LEN`, for example `10.244.1.7/32`. The spec models
    /// this as a plain string, not a structured CIDR, because unlike a pool
    /// its host bits are never zero.
    address: String,
    /// The default gateway for this address, if the attachment set one up.
    #[serde(skip_serializing_if = "Option::is_none")]
    gateway: Option<Ipv4Addr>,
    /// Which entry in [`AddResult::interfaces`] this address belongs to.
    #[serde(skip_serializing_if = "Option::is_none")]
    interface: Option<u32>,
}

impl IpConfig {
    /// Builds an address entry from its parts, formatting `address` and
    /// `prefix_len` into the spec's `ADDRESS/PREFIX_LEN` string.
    #[must_use]
    pub fn new(
        address: Ipv4Addr,
        prefix_len: u8,
        gateway: Option<Ipv4Addr>,
        interface: Option<u32>,
    ) -> Self {
        Self {
            address: format!("{address}/{prefix_len}"),
            gateway,
            interface,
        }
    }
}

/// One route the attachment installed.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Route {
    /// The route's destination network.
    dst: Ipv4Cidr,
    /// The next hop. If unset, a runtime may fall back to an address's
    /// `gateway`.
    #[serde(skip_serializing_if = "Option::is_none")]
    gw: Option<Ipv4Addr>,
    /// The MTU along the path to `dst`, if it differs from the interface's.
    #[serde(skip_serializing_if = "Option::is_none")]
    mtu: Option<u32>,
}

impl Route {
    /// Builds a route entry from its parts.
    #[must_use]
    pub const fn new(dst: Ipv4Cidr, gw: Option<Ipv4Addr>, mtu: Option<u32>) -> Self {
        Self { dst, gw, mtu }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod single_interface {
        use super::*;

        #[test]
        fn matches_the_spec_shape_for_a_routed_slash_32_pod() {
            let result = AddResult::single_interface(
                "1.1.0",
                "eth0",
                "/var/run/netns/test",
                Ipv4Addr::new(10, 244, 1, 7),
                None,
            );

            assert_eq!(
                serde_json::to_value(&result).unwrap(),
                serde_json::json!({
                    "cniVersion": "1.1.0",
                    "interfaces": [
                        {"name": "eth0", "sandbox": "/var/run/netns/test"}
                    ],
                    "ips": [
                        {"address": "10.244.1.7/32", "gateway": "169.254.1.1", "interface": 0}
                    ],
                    "routes": [
                        {"dst": "0.0.0.0/0", "gw": "169.254.1.1"}
                    ],
                })
            );
        }

        #[test]
        fn includes_dns_when_provided() {
            let dns = Dns {
                nameservers: vec!["10.96.0.10".into()],
                ..Dns::default()
            };

            let result = AddResult::single_interface(
                "1.1.0",
                "eth0",
                "/var/run/netns/test",
                Ipv4Addr::new(10, 244, 1, 7),
                Some(dns),
            );

            assert_eq!(
                serde_json::to_value(&result).unwrap()["dns"],
                serde_json::json!({"nameservers": ["10.96.0.10"]})
            );
        }

        #[test]
        fn omits_empty_dns_fields() {
            let dns = Dns {
                nameservers: vec!["10.96.0.10".into()],
                ..Dns::default()
            };

            let json = serde_json::to_value(dns).unwrap();

            assert_eq!(
                json.as_object().unwrap().len(),
                1,
                "expected only nameservers: {json}"
            );
        }
    }

    mod interface {
        use super::*;

        #[test]
        fn omits_mac_mtu_and_sandbox_when_unset() {
            let iface = Interface {
                name: "narrows0".into(),
                mac: None,
                mtu: None,
                sandbox: String::new(),
            };

            assert_eq!(
                serde_json::to_value(&iface).unwrap(),
                serde_json::json!({"name": "narrows0"})
            );
        }

        #[test]
        fn includes_mac_and_mtu_once_known() {
            let iface = Interface {
                name: "eth0".into(),
                mac: Some("00:11:22:33:44:55".into()),
                mtu: Some(1500),
                sandbox: "/var/run/netns/test".into(),
            };

            assert_eq!(
                serde_json::to_value(&iface).unwrap(),
                serde_json::json!({
                    "name": "eth0",
                    "mac": "00:11:22:33:44:55",
                    "mtu": 1500,
                    "sandbox": "/var/run/netns/test",
                })
            );
        }
    }

    mod ip_config {
        use super::*;

        #[test]
        fn formats_address_in_cidr_notation() {
            let ip = IpConfig::new(Ipv4Addr::new(192, 168, 1, 3), 24, None, None);

            assert_eq!(
                serde_json::to_value(&ip).unwrap(),
                serde_json::json!({"address": "192.168.1.3/24"})
            );
        }
    }

    mod route {
        use super::*;

        #[test]
        fn omits_gw_and_mtu_when_unset() {
            let route = Route::new("0.0.0.0/0".parse().unwrap(), None, None);

            assert_eq!(
                serde_json::to_value(&route).unwrap(),
                serde_json::json!({"dst": "0.0.0.0/0"})
            );
        }

        #[test]
        fn includes_gw_and_mtu_when_set() {
            let route = Route::new(
                "0.0.0.0/0".parse().unwrap(),
                Some(Ipv4Addr::new(169, 254, 1, 1)),
                Some(1450),
            );

            assert_eq!(
                serde_json::to_value(&route).unwrap(),
                serde_json::json!({"dst": "0.0.0.0/0", "gw": "169.254.1.1", "mtu": 1450})
            );
        }
    }
}
