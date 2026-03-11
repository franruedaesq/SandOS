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
use embassy_time::{Duration, Instant, Timer};
use esp_wifi::wifi::{
    AuthMethod, ClientConfiguration, Configuration, WifiController, WifiDevice, WifiStaDevice,
};
use portable_atomic::{AtomicU8, AtomicU32};
use core::sync::atomic::Ordering;
use static_cell::StaticCell;

use crate::core0::espnow;
use crate::web_server;

// ---------------------------------------------------------------------------
// WiFi status exported for display UI
// ---------------------------------------------------------------------------

pub const WIFI_STATUS_DISCONNECTED: u8 = 0;
pub const WIFI_STATUS_CONNECTING: u8 = 1;
pub const WIFI_STATUS_CONNECTED: u8 = 2;
pub const WIFI_STATUS_ERROR: u8 = 3;

static WIFI_STATUS: AtomicU8 = AtomicU8::new(WIFI_STATUS_DISCONNECTED);
static WIFI_IPV4_BE: AtomicU32 = AtomicU32::new(0);

#[inline]
pub fn wifi_status() -> u8 {
    WIFI_STATUS.load(Ordering::Relaxed)
}

#[inline]
pub fn wifi_ipv4() -> Option<[u8; 4]> {
    let raw = WIFI_IPV4_BE.load(Ordering::Relaxed);
    if raw == 0 {
        None
    } else {
        Some(raw.to_be_bytes())
    }
}

#[inline]
pub fn mark_connecting() {
    WIFI_STATUS.store(WIFI_STATUS_CONNECTING, Ordering::Relaxed);
    WIFI_IPV4_BE.store(0, Ordering::Relaxed);
}

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
///
/// Runs immediately — the WiFi driver needs the packet processor active to
/// avoid internal buffer overflows and crashes.  Display starvation during
/// DHCP (~30-50 s) is mitigated by keeping the web server disabled at boot.
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
    WIFI_STATUS.store(WIFI_STATUS_DISCONNECTED, Ordering::Relaxed);
    WIFI_IPV4_BE.store(0, Ordering::Relaxed);

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

    let mut client_cfg = ClientConfiguration {
        ssid,
        password,
        ..Default::default()
    };

    if !PASSWORD.is_empty() {
        // Prefer WPA2 association path for better stability on mixed-mode APs.
        client_cfg.auth_method = AuthMethod::WPA2Personal;
    }

    let mut ctrl = controller;

    let apply_sta_config = |ctrl: &mut WifiController<'static>| {
        ctrl.set_configuration(&Configuration::Client(client_cfg.clone()))
    };

    // ------------------------------------------------------------------
    // Pre-configure STA credentials BEFORE the radio starts.
    //
    // new_with_mode() (called in main.rs) already set
    // esp_wifi_set_mode(STA) at controller creation time — the radio is
    // in STA mode but esp_wifi_start() has NOT been called yet.
    // esp_wifi_set_config() is valid before start per the IDF contract.
    // ------------------------------------------------------------------
    log::info!("[wifi] pre-configuring STA credentials (radio not started yet)");
    match apply_sta_config(&mut ctrl) {
        Ok(()) => log::info!("[wifi] STA credentials pre-configured OK"),
        Err(e) => {
            // Mode=STA was set by new_with_mode() so this should never fail.
            log::error!("[wifi] early set_configuration failed: {:?} — task halted", e);
            return;
        }
    }

    // Keep radio idle until the user explicitly enables the Web UI.
    // ESP-NOW will call esp_wifi_start() while we wait — our credentials
    // are already committed and will be picked up at that point.
    while !web_server::is_web_server_enabled() {
        Timer::after_millis(500).await;
    }
    log::info!("[wifi] enabling STA because web server was enabled");

    // ------------------------------------------------------------------
    // If ESP-NOW already started the shared radio, do not stop it.
    // Re-apply STA credentials and proceed directly to connect().
    // Otherwise start STA ourselves.
    // ------------------------------------------------------------------
    if ctrl.is_started().unwrap_or(false) {
        log::info!("[wifi] radio already started by ESP-NOW — applying STA config");
        if let Err(e) = apply_sta_config(&mut ctrl) {
            log::error!("[wifi] set_configuration failed: {:?} — task halted", e);
            WIFI_STATUS.store(WIFI_STATUS_ERROR, Ordering::Relaxed);
            return;
        }
    } else {
        log::info!("[wifi] start -> start");
        let mut start_attempt: u32 = 0;
        'start_loop: loop {
            start_attempt = start_attempt.wrapping_add(1);
            match ctrl.start() {
                Ok(()) => {
                    let start_wait = Instant::now();
                    let mut last_log = start_wait;
                    loop {
                        match ctrl.is_started() {
                            Ok(true) => {
                                let took_ms = (Instant::now() - start_wait).as_millis();
                                log::info!(
                                    "[wifi] start -> done ({}ms, attempt #{})",
                                    took_ms,
                                    start_attempt
                                );
                                break 'start_loop;
                            }
                            Ok(false) => {}
                            Err(e) => {
                                log::warn!(
                                    "[wifi] is_started error on attempt #{}: {:?}",
                                    start_attempt,
                                    e
                                );
                                Timer::after_millis(1_000).await;
                                continue 'start_loop;
                            }
                        }

                        let now = Instant::now();
                        if now - start_wait >= Duration::from_secs(8) {
                            log::warn!(
                                "[wifi] start not confirmed after 8s (attempt #{}) — retrying",
                                start_attempt
                            );
                            Timer::after_millis(1_000).await;
                            continue 'start_loop;
                        }

                        if now - last_log >= Duration::from_secs(2) {
                            let elapsed = (now - start_wait).as_secs();
                            log::info!(
                                "[wifi] waiting for STA start ({}s, attempt #{})",
                                elapsed,
                                start_attempt
                            );
                            last_log = now;
                        }
                        Timer::after_millis(250).await;
                    }
                }
                Err(e) => {
                    WIFI_STATUS.store(WIFI_STATUS_ERROR, Ordering::Relaxed);
                    log::warn!(
                        "[wifi] start failed on attempt #{}: {:?} — retrying in 2 s",
                        start_attempt,
                        e
                    );
                    Timer::after_millis(2_000).await;
                }
            }
        }
    }

    // Isolate first STA association from ESP-NOW traffic.
    log::info!("[wifi] pausing ESP-NOW I/O for STA association");
    espnow::set_espnow_io_paused(true);
    let mut espnow_paused_for_first_assoc = true;
    // Give espnow_rx_task time to leave any in-flight receive/send path.
    Timer::after_millis(1_200).await;


    log::info!("[wifi] connecting to '{}'…", SSID);

    // Safety guard: a number of esp-wifi/IDF combinations can hit unstable
    // disconnect callback paths when attempting to associate to a secured AP
    // with an empty password. Validate against scan results first.
    if PASSWORD.is_empty() {
        log::warn!(
            "[wifi] WIFI_PASSWORD is empty — validating '{}' auth mode before connect",
            SSID
        );
        match ctrl.scan_n::<20>() {
            Ok((aps, _total)) => {
                if let Some(ap) = aps.iter().find(|ap| ap.ssid.as_str() == SSID) {
                    if ap.auth_method != Some(AuthMethod::None) {
                        if espnow_paused_for_first_assoc {
                            espnow::set_espnow_io_paused(false);
                        }
                        WIFI_STATUS.store(WIFI_STATUS_ERROR, Ordering::Relaxed);
                        log::error!(
                            "[wifi] SSID '{}' requires {:?}, but WIFI_PASSWORD is empty — refusing connect",
                            SSID,
                            ap.auth_method
                        );
                        return;
                    }
                } else {
                    log::warn!(
                        "[wifi] SSID '{}' not visible in scan; continuing without auth pre-check",
                        SSID
                    );
                }
            }
            Err(e) => {
                log::warn!(
                    "[wifi] pre-connect scan failed: {:?} — continuing without auth pre-check",
                    e
                );
            }
        }
    }

    let mut connect_attempt: u32 = 0;
    // Tracks back-to-back WifiError::Disconnected results.  Three or more
    // in a row almost always means wrong password or AP is rejecting us —
    // surface a clear diagnostic rather than silently looping.
    let mut consecutive_auth_failures: u32 = 0;

    'connect_loop: loop {
        connect_attempt = connect_attempt.wrapping_add(1);
        WIFI_STATUS.store(WIFI_STATUS_CONNECTING, Ordering::Relaxed);
        WIFI_IPV4_BE.store(0, Ordering::Relaxed);
        let connect_started = Instant::now();
        log::info!("[wifi] connect attempt #{}", connect_attempt);
        if let Err(e) = ctrl.connect() {
            WIFI_STATUS.store(WIFI_STATUS_ERROR, Ordering::Relaxed);
            log::warn!(
                "[wifi] connect start failed on attempt #{}: {:?} — retrying in 5 s",
                connect_attempt,
                e
            );
            Timer::after_millis(5_000).await;
            continue;
        }

        // Wait until associated or disconnected/error.
        let mut last_assoc_log = connect_started;
        loop {
            match ctrl.is_connected() {
                Ok(true) => {
                    let took_ms = (Instant::now() - connect_started).as_millis();
                    log::info!("[wifi] associated ({}ms)", took_ms);
                    consecutive_auth_failures = 0; // reset on success
                    break;
                }
                Ok(false) => {}
                Err(esp_wifi::wifi::WifiError::Disconnected) => {
                    consecutive_auth_failures =
                        consecutive_auth_failures.wrapping_add(1);
                    WIFI_STATUS.store(WIFI_STATUS_ERROR, Ordering::Relaxed);
                    log::warn!(
                        "[wifi] AP rejected connection on attempt #{} \
                         (consecutive auth failures: {})",
                        connect_attempt,
                        consecutive_auth_failures,
                    );
                    if consecutive_auth_failures >= 3 {
                        log::error!(
                            "[wifi] {} consecutive rejections — \
                             likely wrong password or SSID for '{}'. \
                             Rebuild with correct WIFI_PASSWORD env var.",
                            consecutive_auth_failures,
                            SSID,
                        );
                    }
                    Timer::after_millis(5_000).await;
                    continue 'connect_loop;
                }
                Err(e) => {
                    consecutive_auth_failures = 0;
                    WIFI_STATUS.store(WIFI_STATUS_ERROR, Ordering::Relaxed);
                    log::warn!(
                        "[wifi] connect error on attempt #{}: {:?} — retrying in 5 s",
                        connect_attempt,
                        e
                    );
                    Timer::after_millis(5_000).await;
                    continue 'connect_loop;
                }
            }

            let now = Instant::now();
            if now - connect_started >= Duration::from_secs(20) {
                WIFI_STATUS.store(WIFI_STATUS_ERROR, Ordering::Relaxed);
                log::warn!(
                    "[wifi] connect timeout on attempt #{} (20s) — \
                     AP not found or not responding",
                    connect_attempt
                );
                let _ = ctrl.disconnect();
                Timer::after_millis(2_000).await;
                continue 'connect_loop;
            }

            if now - last_assoc_log >= Duration::from_secs(5) {
                let elapsed = (now - connect_started).as_secs();
                log::info!(
                    "[wifi] waiting for association ({}s, attempt #{})",
                    elapsed,
                    connect_attempt
                );
                last_assoc_log = now;
            }

            Timer::after_millis(250).await;
        }

        // Wait for DHCP lease.
        log::info!("[wifi] waiting for DHCP…");
        let dhcp_wait_started = Instant::now();
        let mut last_dhcp_log = dhcp_wait_started;
        loop {
            if let Some(config) = stack.config_v4() {
                let ip = config.address.address();
                let octets = ip.octets();
                WIFI_IPV4_BE.store(u32::from_be_bytes(octets), Ordering::Relaxed);
                WIFI_STATUS.store(WIFI_STATUS_CONNECTED, Ordering::Relaxed);
                if espnow_paused_for_first_assoc {
                    espnow::set_espnow_io_paused(false);
                    espnow_paused_for_first_assoc = false;
                    log::info!("[wifi] resuming ESP-NOW I/O after STA association");
                }
                log::info!(
                    "[wifi] IP: {}.{}.{}.{}",
                    octets[0], octets[1], octets[2], octets[3]
                );
                log::info!("[wifi] Web UI → http://{}.{}.{}.{}/", octets[0], octets[1], octets[2], octets[3]);
                break;
            }

            if !matches!(ctrl.is_connected(), Ok(true)) {
                WIFI_STATUS.store(WIFI_STATUS_DISCONNECTED, Ordering::Relaxed);
                WIFI_IPV4_BE.store(0, Ordering::Relaxed);
                log::warn!("[wifi] link dropped before DHCP lease — reconnecting…");
                break;
            }

            let now = Instant::now();
            if now - last_dhcp_log >= Duration::from_secs(5) {
                let elapsed = (now - dhcp_wait_started).as_secs();
                log::info!("[wifi] still waiting for DHCP ({}s)", elapsed);
                last_dhcp_log = now;
            }
            Timer::after_millis(500).await;
        }

        if WIFI_STATUS.load(Ordering::Relaxed) != WIFI_STATUS_CONNECTED {
            Timer::after_millis(1_000).await;
            continue;
        }

        // Monitor link — reconnect on drop.
        loop {
            if matches!(ctrl.is_connected(), Ok(true)) {
                Timer::after_millis(2_000).await;
            } else {
                WIFI_STATUS.store(WIFI_STATUS_DISCONNECTED, Ordering::Relaxed);
                WIFI_IPV4_BE.store(0, Ordering::Relaxed);
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
