//! LAN discovery over mDNS (`_devbridge._tcp.local.`).
//!
//! Advertises only this device's id and port. While a pairing window is open
//! it also advertises the device name, so the other computer can show
//! "Join Studio Mac" instead of an opaque id.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};

use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use tokio::sync::mpsc;

pub const SERVICE: &str = "_devbridge._tcp.local.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovered {
    pub device_id: String,
    /// Present only while that device has a pairing window open.
    pub pairing_name: Option<String>,
    pub addrs: Vec<SocketAddr>,
}

pub struct Discovery {
    daemon: ServiceDaemon,
    device_id: String,
    port: u16,
    fullname: Option<String>,
}

impl Discovery {
    /// Starts browsing; resolved peers are sent to `tx`.
    pub fn start(device_id: &str, port: u16, tx: mpsc::UnboundedSender<Discovered>) -> Result<Self, mdns_sd::Error> {
        let daemon = ServiceDaemon::new()?;
        let rx = daemon.browse(SERVICE)?;
        let me = device_id.to_owned();
        std::thread::Builder::new()
            .name("devbridge-mdns".into())
            .spawn(move || {
                while let Ok(ev) = rx.recv() {
                    if let ServiceEvent::ServiceResolved(info) = ev {
                        let Some(id) = info.get_property_val_str("id").map(str::to_owned) else { continue };
                        if id == me {
                            continue;
                        }
                        let mut addrs: Vec<SocketAddr> = info
                            .get_addresses()
                            .iter()
                            .map(|a| a.to_ip_addr())
                            .filter(|ip| !ip.is_loopback() && !is_link_local(ip))
                            .map(|ip| SocketAddr::new(ip, info.port))
                            .collect();
                        addrs.sort_by_key(|a| !a.is_ipv4()); // prefer IPv4
                        let pairing_name = info.get_property_val_str("name").map(str::to_owned);
                        if tx.send(Discovered { device_id: id, pairing_name, addrs }).is_err() {
                            break;
                        }
                    }
                }
            })
            .map_err(|e| mdns_sd::Error::Msg(e.to_string()))?;
        let mut d = Self { daemon, device_id: device_id.to_owned(), port, fullname: None };
        d.advertise(None)?;
        Ok(d)
    }

    /// (Re-)registers this device; `pairing_name` is included only while pairing.
    pub fn advertise(&mut self, pairing_name: Option<&str>) -> Result<(), mdns_sd::Error> {
        if let Some(f) = self.fullname.take() {
            let _ = self.daemon.unregister(&f);
        }
        let mut props = HashMap::new();
        props.insert("id".to_string(), self.device_id.clone());
        props.insert("v".to_string(), "1".to_string());
        if let Some(n) = pairing_name {
            props.insert("name".to_string(), n.chars().take(64).collect());
        }
        let host = format!("devbridge-{}.local.", &self.device_id.to_lowercase()[..12.min(self.device_id.len())]);
        let info = ServiceInfo::new(SERVICE, &self.device_id, &host, "", self.port, props)?.enable_addr_auto();
        self.fullname = Some(info.get_fullname().to_owned());
        self.daemon.register(info)
    }
}

impl Drop for Discovery {
    fn drop(&mut self) {
        if let Some(f) = self.fullname.take() {
            let _ = self.daemon.unregister(&f);
        }
        let _ = self.daemon.shutdown();
    }
}

fn is_link_local(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_link_local(),
        IpAddr::V6(v6) => (v6.segments()[0] & 0xffc0) == 0xfe80,
    }
}
