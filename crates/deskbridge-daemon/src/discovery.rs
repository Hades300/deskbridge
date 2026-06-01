//! Zero-configuration LAN discovery over mDNS/DNS-SD.
//!
//! A server advertises a `_deskbridge._tcp.local.` service so clients can find
//! it without anyone typing an IP address. The TXT record carries the server's
//! screen name and version so a UI can show a friendly picker.

use anyhow::{Context, Result};
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use std::{
    collections::BTreeMap,
    net::{IpAddr, SocketAddr},
    time::{Duration, Instant},
};

/// DNS-SD service type for DeskBridge servers.
pub const SERVICE_TYPE: &str = "_deskbridge._tcp.local.";

/// A registered advertisement. Dropping it unregisters the service and shuts
/// the mDNS daemon down, so callers keep it alive for the server's lifetime.
pub struct ServiceHandle {
    daemon: ServiceDaemon,
    fullname: String,
}

impl Drop for ServiceHandle {
    fn drop(&mut self) {
        let _ = self.daemon.unregister(&self.fullname);
        let _ = self.daemon.shutdown();
    }
}

/// Advertise a DeskBridge server on the local network.
///
/// `port` is the TCP port the server listens on. Interface addresses are
/// detected automatically, so a server bound to `0.0.0.0` still advertises its
/// real LAN addresses.
pub fn register(screen_name: &str, port: u16) -> Result<ServiceHandle> {
    let daemon = ServiceDaemon::new().context("failed to start mDNS daemon")?;

    let host = sanitize_hostname(screen_name);
    let host_name = format!("{host}.local.");
    let instance = format!("DeskBridge {screen_name}");
    let proto = deskbridge_core::PROTOCOL_VERSION.to_string();
    let properties = [
        ("name", screen_name),
        ("version", crate::build_info::version()),
        ("proto", proto.as_str()),
    ];

    // Empty address string + enable_addr_auto() lets the library track the
    // host's real interface addresses, including across network changes.
    let info = ServiceInfo::new(
        SERVICE_TYPE,
        &instance,
        &host_name,
        "",
        port,
        &properties[..],
    )
    .context("invalid mDNS service info")?
    .enable_addr_auto();
    let fullname = info.get_fullname().to_string();
    daemon
        .register(info)
        .context("failed to register mDNS service")?;

    Ok(ServiceHandle { daemon, fullname })
}

/// A DeskBridge server found on the network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredServer {
    pub name: String,
    pub addr: SocketAddr,
    pub version: Option<String>,
}

/// Browse the local network for DeskBridge servers for up to `timeout`.
///
/// Returns one entry per resolved `addr:port`, de-duplicated and sorted so the
/// output is stable for a UI list or CLI table.
pub fn discover(timeout: Duration) -> Result<Vec<DiscoveredServer>> {
    let daemon = ServiceDaemon::new().context("failed to start mDNS daemon")?;
    let receiver = daemon
        .browse(SERVICE_TYPE)
        .context("failed to browse for DeskBridge services")?;

    let deadline = Instant::now() + timeout;
    let mut found: BTreeMap<SocketAddr, DiscoveredServer> = BTreeMap::new();

    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        match receiver.recv_timeout(remaining) {
            Ok(ServiceEvent::ServiceResolved(info)) => {
                let port = info.get_port();
                let name = info
                    .get_property("name")
                    .map(|prop| prop.val_str().to_string())
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| trim_instance(info.get_fullname()));
                let version = info
                    .get_property("version")
                    .map(|prop| prop.val_str().to_string())
                    .filter(|version| !version.is_empty());

                for ip in info.get_addresses_v4() {
                    let addr = SocketAddr::new(IpAddr::V4(ip), port);
                    found.entry(addr).or_insert_with(|| DiscoveredServer {
                        name: name.clone(),
                        addr,
                        version: version.clone(),
                    });
                }
            }
            Ok(_) => continue,
            Err(_) => break,
        }
    }

    let _ = daemon.shutdown();
    Ok(found.into_values().collect())
}

/// mDNS host labels must avoid `.` (it separates labels) and whitespace.
fn sanitize_hostname(name: &str) -> String {
    let cleaned = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();
    let trimmed = cleaned.trim_matches('-');
    if trimmed.is_empty() {
        "deskbridge".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Best-effort fallback name from a fullname like
/// `DeskBridge mac._deskbridge._tcp.local.`.
fn trim_instance(fullname: &str) -> String {
    fullname
        .split('.')
        .next()
        .unwrap_or(fullname)
        .trim_start_matches("DeskBridge ")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_hostnames() {
        assert_eq!(sanitize_hostname("mac"), "mac");
        assert_eq!(sanitize_hostname("my mac.local"), "my-mac-local");
        assert_eq!(sanitize_hostname("..."), "deskbridge");
        assert_eq!(sanitize_hostname(""), "deskbridge");
    }

    #[test]
    fn trims_instance_names() {
        assert_eq!(
            trim_instance("DeskBridge mac._deskbridge._tcp.local."),
            "mac"
        );
    }
}
