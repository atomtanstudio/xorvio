//! Lists the Chromecasts and Google TVs on this network, by browsing mDNS
//! for ten seconds. It never connects to one, so nothing turns on.
//!
//!     cargo run -p xorvio-cast --example discover

use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let discovery = xorvio_cast::Discovery::start()?;
    tokio::time::sleep(Duration::from_secs(10)).await;
    let devices = discovery.devices();
    if devices.is_empty() {
        println!("No TVs found. (On macOS, Xorvio or the terminal needs Local Network access in System Settings › Privacy & Security.)");
    }
    for device in devices {
        println!("{} ({})", device.name, device.model.as_deref().unwrap_or("unknown model"));
    }
    Ok(())
}
