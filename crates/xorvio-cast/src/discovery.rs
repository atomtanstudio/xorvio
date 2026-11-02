//! Finding Chromecasts and Google TVs: they announce `_googlecast._tcp` over
//! mDNS with their name, model and what they can do in the TXT record.
//! Browsing only listens and asks who's there; nothing connects to a device
//! until someone chooses it.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};

use mdns_sd::{IfKind, ServiceDaemon, ServiceEvent};
use serde::Serialize;
use tokio::sync::watch;

const SERVICE: &str = "_googlecast._tcp.local.";
/// The `ca` bit for devices that show video. Speakers and speaker groups
/// lack it.
const VIDEO_OUT: u32 = 1;

/// A device that can show video, as the interface lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    /// The device's own id, which stays the same across restarts.
    pub id: String,
    /// The name its owner gave it ("Living Room TV").
    pub name: String,
    /// "Chromecast", "Google TV Streamer", a TV's model…
    pub model: Option<String>,
    #[serde(skip)]
    pub addresses: Vec<IpAddr>,
    #[serde(skip)]
    pub port: u16,
}

impl Device {
    /// Where to connect: an IPv4 address when it has one, since that's
    /// what the stream's address will be on too.
    pub fn address(&self) -> Option<SocketAddr> {
        let ip = self.addresses.iter().find(|ip| ip.is_ipv4()).or_else(|| self.addresses.first())?;
        Some(SocketAddr::new(*ip, self.port))
    }
}

/// Reads a device from what mDNS resolved, or `None` for one that can't
/// show video (a speaker, a speaker group) or says too little.
pub(crate) fn device_from(fullname: &str, port: u16, mut addresses: Vec<IpAddr>, txt: impl Fn(&str) -> Option<String>) -> Option<Device> {
    let model = txt("md").filter(|m| !m.trim().is_empty());
    if model.as_deref() == Some("Google Cast Group") {
        return None;
    }
    if let Some(capabilities) = txt("ca").and_then(|ca| ca.trim().parse::<u32>().ok())
        && capabilities & VIDEO_OUT == 0
    {
        return None;
    }
    if addresses.is_empty() || port == 0 {
        return None;
    }
    addresses.sort_by_key(|ip| (!ip.is_ipv4(), *ip));
    let instance = fullname.strip_suffix(SERVICE).unwrap_or(fullname).trim_end_matches('.');
    let name = txt("fn").map(|n| n.trim().to_string()).filter(|n| !n.is_empty()).unwrap_or_else(|| instance.to_string());
    let id = txt("id").map(|id| id.trim().to_string()).filter(|id| !id.is_empty()).unwrap_or_else(|| instance.to_string());
    Some(Device { id, name, model, addresses, port })
}

/// Browsing for devices, for as long as it's kept.
pub struct Discovery {
    daemon: ServiceDaemon,
    devices: watch::Receiver<Vec<Device>>,
    task: tokio::task::AbortHandle,
}

impl Discovery {
    /// Starts browsing; devices appear in [`Discovery::devices`] as they
    /// answer. Needs a tokio runtime.
    pub fn start() -> anyhow::Result<Discovery> {
        Discovery::browse(daemon()?)
    }

    /// Browsing this computer's loopback only, for tests that announce a
    /// pretend device without it reaching the network.
    #[doc(hidden)]
    pub fn start_on_loopback() -> anyhow::Result<Discovery> {
        let daemon = daemon()?;
        daemon.disable_interface(IfKind::All)?;
        daemon.enable_interface(IfKind::LoopbackV4)?;
        Discovery::browse(daemon)
    }

    fn browse(daemon: ServiceDaemon) -> anyhow::Result<Discovery> {
        let events = daemon.browse(SERVICE).map_err(|error| {
            tracing::warn!("mDNS browsing couldn't start: {error}");
            anyhow::anyhow!("Xorvio couldn't look for TVs on this network.")
        })?;
        let (sender, devices) = watch::channel(Vec::new());
        let task = tokio::spawn(async move {
            // By mDNS name, which removals go by.
            let mut found: HashMap<String, Device> = HashMap::new();
            while let Ok(event) = events.recv_async().await {
                let changed = match event {
                    ServiceEvent::ServiceResolved(service) => {
                        let addresses = service.get_addresses().iter().map(|ip| ip.to_ip_addr()).collect();
                        let device = device_from(service.get_fullname(), service.get_port(), addresses, |key| service.get_property_val_str(key).map(String::from));
                        match device {
                            Some(device) if found.get(service.get_fullname()) != Some(&device) => {
                                tracing::info!(name = %device.name, model = ?device.model, "found a cast device");
                                found.insert(service.get_fullname().to_string(), device);
                                true
                            }
                            _ => false,
                        }
                    }
                    ServiceEvent::ServiceRemoved(_, fullname) => found.remove(&fullname).is_some(),
                    _ => false,
                };
                if changed {
                    let mut list: Vec<Device> = found.values().cloned().collect();
                    // A device seen under two mDNS names (a new name after a restart) is listed once.
                    list.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.id.cmp(&b.id)));
                    list.dedup_by(|a, b| a.id == b.id);
                    sender.send_replace(list);
                }
            }
        });
        Ok(Discovery { daemon, devices, task: task.abort_handle() })
    }

    /// The devices found so far, by name.
    pub fn devices(&self) -> Vec<Device> {
        self.devices.borrow().clone()
    }

    /// Changes to the list.
    pub fn changes(&self) -> watch::Receiver<Vec<Device>> {
        self.devices.clone()
    }

    pub fn device(&self, id: &str) -> Option<Device> {
        self.devices.borrow().iter().find(|d| d.id == id).cloned()
    }
}

fn daemon() -> anyhow::Result<ServiceDaemon> {
    ServiceDaemon::new().map_err(|error| {
        tracing::warn!("mDNS couldn't start: {error}");
        anyhow::anyhow!("Xorvio couldn't look for TVs on this network.")
    })
}

impl Drop for Discovery {
    fn drop(&mut self) {
        self.task.abort();
        let _ = self.daemon.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn txt<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |key| pairs.iter().find(|(k, _)| *k == key).map(|(_, v)| v.to_string())
    }

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    #[test]
    fn reads_a_chromecast() {
        let fields = [("id", "4d7c2f0e9a8b"), ("fn", "Living Room TV"), ("md", "Chromecast"), ("ca", "4101")];
        let device = device_from("Chromecast-4d7c2f0e9a8b._googlecast._tcp.local.", 8009, vec![ip("fe80::1"), ip("192.168.1.30")], txt(&fields)).unwrap();
        assert_eq!(device.id, "4d7c2f0e9a8b");
        assert_eq!(device.name, "Living Room TV");
        assert_eq!(device.model.as_deref(), Some("Chromecast"));
        assert_eq!(device.address(), Some("192.168.1.30:8009".parse().unwrap()), "IPv4 first");
    }

    #[test]
    fn leaves_out_speakers_and_groups() {
        let speaker = [("id", "a"), ("fn", "Kitchen speaker"), ("md", "Google Home"), ("ca", "2052")];
        assert!(device_from("x", 8009, vec![ip("192.168.1.31")], txt(&speaker)).is_none());
        let group = [("id", "b"), ("fn", "Downstairs"), ("md", "Google Cast Group"), ("ca", "4101")];
        assert!(device_from("x", 32187, vec![ip("192.168.1.31")], txt(&group)).is_none());
        let unknown = [("id", "c"), ("fn", "Bedroom")];
        assert!(device_from("x", 8009, vec![ip("192.168.1.32")], txt(&unknown)).is_some(), "no capabilities said: worth offering");
        assert!(device_from("x", 8009, vec![], txt(&unknown)).is_none(), "no address to reach");
    }

    #[test]
    fn names_come_from_the_instance_when_missing() {
        let device = device_from("Bedroom-TV._googlecast._tcp.local.", 8009, vec![ip("10.0.0.5")], |_| None).unwrap();
        assert_eq!((device.name.as_str(), device.id.as_str()), ("Bedroom-TV", "Bedroom-TV"));
    }
}
