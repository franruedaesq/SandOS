#![allow(dead_code)]
//! Minimal SNTP client for SandOS.
//!
//! Syncs the wall clock once on startup (after WiFi connects), then
//! re-syncs every 30 minutes.  The offset is stored in atomics so any
//! task can call `wall_clock_secs()` without locking.

use embassy_executor::task;
use embassy_net::{udp::{PacketMetadata, UdpSocket}, Stack};
use embassy_time::{Duration, Instant, Timer};
use portable_atomic::{AtomicBool, AtomicI64};
use core::sync::atomic::Ordering;

/// Offset in milliseconds: NTP wall-clock time minus `Instant::now().as_millis()`.
static NTP_OFFSET_MS: AtomicI64 = AtomicI64::new(0);

/// True once the first successful sync has completed.
static NTP_SYNCED: AtomicBool = AtomicBool::new(false);

/// Return the current Unix timestamp (seconds since 1970-01-01) if NTP
/// has synced at least once.  Returns `None` before the first sync.
pub fn wall_clock_secs() -> Option<u64> {
    if !NTP_SYNCED.load(Ordering::Acquire) {
        return None;
    }
    let offset = NTP_OFFSET_MS.load(Ordering::Acquire);
    let now_ms = Instant::now().as_millis() as i64;
    Some(((now_ms + offset) / 1000) as u64)
}

/// NTP server: time.google.com (216.239.35.0)
const NTP_SERVER: embassy_net::IpAddress =
    embassy_net::IpAddress::Ipv4(embassy_net::Ipv4Address::new(216, 239, 35, 0));
const NTP_PORT: u16 = 123;

/// Seconds between 1900-01-01 and 1970-01-01 (NTP epoch offset).
const NTP_TO_UNIX_OFFSET: u64 = 2_208_988_800;

/// Re-sync interval.
const RESYNC_INTERVAL: Duration = Duration::from_secs(30 * 60);

#[task]
pub async fn ntp_sync_task(stack: &'static Stack<'static>) {
    // Wait for WiFi to be connected and have an IP.
    loop {
        if crate::wifi::wifi_status() == crate::wifi::WIFI_STATUS_CONNECTED {
            break;
        }
        Timer::after(Duration::from_secs(2)).await;
    }
    log::info!("[ntp] WiFi connected — starting NTP sync");

    // Allow some time for network to stabilise.
    Timer::after(Duration::from_secs(2)).await;

    loop {
        if let Err(e) = do_ntp_sync(stack).await {
            log::warn!("[ntp] sync failed: {}", e);
            Timer::after(Duration::from_secs(30)).await;
            continue;
        }
        Timer::after(RESYNC_INTERVAL).await;
    }
}

async fn do_ntp_sync(stack: &'static Stack<'static>) -> Result<(), &'static str> {
    let mut rx_meta = [PacketMetadata::EMPTY; 2];
    let mut rx_buf = [0u8; 256];
    let mut tx_meta = [PacketMetadata::EMPTY; 2];
    let mut tx_buf = [0u8; 256];
    let mut socket = UdpSocket::new(
        *stack,
        &mut rx_meta,
        &mut rx_buf,
        &mut tx_meta,
        &mut tx_buf,
    );

    socket.bind(0).map_err(|_| "bind failed")?;

    // Build minimal SNTP request (48 bytes).
    // LI=0, VN=4, Mode=3 (client) → first byte = 0b00_100_011 = 0x23
    let mut request = [0u8; 48];
    request[0] = 0x23;

    let endpoint = embassy_net::IpEndpoint::new(NTP_SERVER, NTP_PORT);
    socket.send_to(&request, endpoint).await.map_err(|_| "send failed")?;

    // Record the local time just before waiting for response.
    let send_time_ms = Instant::now().as_millis() as i64;

    // Wait for response with a 5-second timeout.
    let mut response = [0u8; 48];
    let result = embassy_time::with_timeout(
        Duration::from_secs(5),
        socket.recv_from(&mut response),
    )
    .await;

    let recv_time_ms = Instant::now().as_millis() as i64;

    match result {
        Ok(Ok((len, _addr))) => {
            if len < 48 {
                return Err("response too short");
            }
        }
        Ok(Err(_)) => return Err("recv error"),
        Err(_) => return Err("timeout"),
    }

    // Extract transmit timestamp (bytes 40-43 = seconds since 1900).
    let ntp_secs = u32::from_be_bytes([response[40], response[41], response[42], response[43]]);
    if ntp_secs == 0 {
        return Err("zero timestamp");
    }

    // Convert NTP timestamp to Unix milliseconds.
    let unix_secs = (ntp_secs as u64).saturating_sub(NTP_TO_UNIX_OFFSET);
    let ntp_ms = (unix_secs * 1000) as i64;

    // Compute offset: wall_clock_ms = local_ms + offset
    // Use the midpoint of send/recv as the local reference.
    let local_ref_ms = (send_time_ms + recv_time_ms) / 2;
    let offset = ntp_ms - local_ref_ms;

    NTP_OFFSET_MS.store(offset, Ordering::Release);
    NTP_SYNCED.store(true, Ordering::Release);

    log::info!(
        "[ntp] synced — Unix epoch {} (offset {}ms)",
        unix_secs,
        offset
    );
    Ok(())
}
