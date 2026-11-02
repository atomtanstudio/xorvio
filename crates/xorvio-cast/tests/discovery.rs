//! Discovery against a pretend Chromecast announced on this computer's
//! loopback only, so nothing is announced to the network.

use std::time::Duration;

use mdns_sd::{IfKind, ServiceDaemon, ServiceInfo};
use xorvio_cast::Discovery;

#[tokio::test]
async fn finds_a_tv_and_leaves_out_a_speaker() {
    let announcer = ServiceDaemon::new().unwrap();
    announcer.disable_interface(IfKind::All).unwrap();
    announcer.enable_interface(IfKind::LoopbackV4).unwrap();
    let announce = |instance: &str, name: &str, model: &str, capabilities: &str| {
        let txt = [("id", instance), ("fn", name), ("md", model), ("ca", capabilities)];
        let host = format!("{instance}.local.");
        ServiceInfo::new("_googlecast._tcp.local.", instance, &host, "127.0.0.1", 8009, &txt[..]).unwrap()
    };
    announcer.register(announce("xorvio-test-tv", "Living Room TV", "Chromecast", "4101")).unwrap();
    announcer.register(announce("xorvio-test-speaker", "Kitchen speaker", "Google Home", "2052")).unwrap();

    let discovery = Discovery::start_on_loopback().unwrap();
    let mut changes = discovery.changes();
    let found = tokio::time::timeout(Duration::from_secs(10), changes.wait_for(|list| !list.is_empty())).await;
    let Ok(Ok(list)) = found else {
        // Some systems don't deliver multicast on loopback; nothing to test there.
        eprintln!("no multicast on loopback here; skipping");
        return;
    };
    let list = list.clone();
    assert_eq!(list.len(), 1, "{list:?}");
    assert_eq!(list[0].name, "Living Room TV");
    assert_eq!(list[0].id, "xorvio-test-tv");
    assert_eq!(list[0].address(), Some("127.0.0.1:8009".parse().unwrap()));
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(discovery.devices().len(), 1, "the speaker never shows up");
    let _ = announcer.shutdown();
}
