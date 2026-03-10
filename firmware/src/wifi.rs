//! WiFi STA connection tasks for SandOS.
//!
//! Connects the ESP32-S3 to a WiFi access point using `esp-wifi` and
//! `embassy-net` with DHCP.  Once an IP address is obtained the web server
//! task can accept HTTP connections.
//!
//! ## Credentials
//!
//! Set at compile time via environment variables:
//! ```sh
//! WIFI_SSID="MyNetwork" WIFI_PASSWORD="secret" cargo run --release
//! ```
//! Defaults to `"wokwi-2"` / `""` (Wokwi's built-in access point) so
//! simulation works without any configuration.

use embassy_executor::task;
use embassy_net::{Runner, Stack, StackResources};
use embassy_time::Timer;
use esp_wifi::wifi::{
    ClientConfiguration, Configuration, WifiController, WifiDevice, WifiStaDevice,
};
use static_cell::StaticCell;

// ---------------------------------------------------------------------------
// Compile-time WiFi credentials
// ---------------------------------------------------------------------------

/// Target SSID – override via environment variable at build time.
const SSID: &str = match option_env!("WIFI_SSID") {
    Some(s) => s,
    None => "wokwi-2",
};

/// WPA2 passphrase – override via environment variable at build time.
const PASSWORD: &str = match option_env!("WIFI_PASSWORD") {
    Some(s) => s,
    None => "",
};

// ---------------------------------------------------------------------------
// Static allocations required by embassy-net
// ---------------------------------------------------------------------------

static STACK_RESOURCES: StaticCell<StackResources<4>> = StaticCell::new();

// ---------------------------------------------------------------------------
// Tasks
// ---------------------------------------------------------------------------

/// Drive the `embassy-net` stack forward (must be co-spawned with `wifi_task`).
#[task]
pub async fn net_task(mut runner: Runner<'static, WifiDevice<'static, WifiStaDevice>>) {
    runner.run().await
}

/// Manage the WiFi connection life-cycle:
/// 1. Configure the interface in station mode.
/// 2. Associate with [`SSID`] / [`PASSWORD`].
/// 3. Wait for DHCP to assign an IPv4 address.
/// 4. Monitor the link and reconnect on disconnect.
#[task]
pub async fn wifi_task(controller: WifiController<'static>, stack: &'static Stack<'static>) {
    log::info!("[wifi] task started — SSID: {}", SSID);

    let ssid = match heapless::String::try_from(SSID) {
        Ok(s) => s,
        Err(_) => {
            log::error!("[wifi] SSID too long — task halted");
            return;
        }
    };
    let password = match heapless::String::try_from(PASSWORD) {
        Ok(p) => p,
        Err(_) => {
            log::error!("[wifi] PASSWORD too long — task halted");
            return;
        }
    };

    let client_cfg = ClientConfiguration {
        ssid,
        password,
        ..Default::default()
    };

    let mut ctrl = controller;
    if let Err(e) = ctrl.set_configuration(&Configuration::Client(client_cfg)) {
        log::error!("[wifi] set_configuration failed: {:?}", e);
        return;
    }
    if let Err(e) = ctrl.start_async().await {
        log::error!("[wifi] start_async failed: {:?}", e);
        return;
    }
    log::info!("[wifi] connecting to '{}'…", SSID);

    loop {
        match ctrl.connect_async().await {
            Ok(()) => log::info!("[wifi] associated"),
            Err(e) => {
                log::warn!("[wifi] connect error: {:?} — retrying in 5 s", e);
                Timer::after_millis(5_000).await;
                continue;
            }
        }

        // Wait for DHCP lease.
        log::info!("[wifi] waiting for DHCP…");
        loop {
            if let Some(config) = stack.config_v4() {
                let ip = config.address.address();
                let octets = ip.octets();
                log::info!(
                    "[wifi] IP: {}.{}.{}.{}",
                    octets[0], octets[1], octets[2], octets[3]
                );
                log::info!("[wifi] Web UI → http://{}.{}.{}.{}/", octets[0], octets[1], octets[2], octets[3]);
                break;
            }
            Timer::after_millis(500).await;
        }

        // Monitor link — reconnect on drop.
        loop {
            if matches!(ctrl.is_connected(), Ok(true)) {
                Timer::after_millis(2_000).await;
            } else {
                log::warn!("[wifi] disconnected — reconnecting…");
                break;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helper used by main to allocate the embassy-net stack
// ---------------------------------------------------------------------------

/// Allocate and return a `(Stack, Runner)` pair for the WiFi STA interface.
///
/// Call this exactly once from `main()`, then spawn [`net_task`] and
/// [`wifi_task`] with the returned values.
pub fn make_stack(
    wifi_interface: WifiDevice<'static, WifiStaDevice>,
) -> (&'static Stack<'static>, Runner<'static, WifiDevice<'static, WifiStaDevice>>) {
    use embassy_net::Config as NetConfig;
    use static_cell::StaticCell;

    static STACK: StaticCell<embassy_net::Stack<'static>> = StaticCell::new();

    let net_config = NetConfig::dhcpv4(Default::default());
    let (stack, runner) = embassy_net::new(
        wifi_interface,
        net_config,
        STACK_RESOURCES.init(StackResources::new()),
        embassy_time::Instant::now().as_ticks(),
    );
    let stack: &'static Stack<'static> = &*STACK.init(stack);
    (stack, runner)
}
