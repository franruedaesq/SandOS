//! ESP-NOW receiver and intent listener
//! Parses incoming JSON payloads and routes them directly to the display task.

use core::sync::atomic::Ordering;
use portable_atomic::AtomicU64;

use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Sender};
use esp_wifi::{
    esp_now::{EspNow, EspNowWithWifiCreateToken, BROADCAST_ADDRESS},
    EspWifiController,
};

// ── Radio link state ──────────────────────────────────────────────────────────

pub static RADIO_LAST_RX_MS: AtomicU64 = AtomicU64::new(0);

#[inline]
pub fn is_radio_link_alive(now_ms: u64) -> bool {
    let last = RADIO_LAST_RX_MS.load(Ordering::Acquire);
    now_ms.saturating_sub(last) < 2000
}

const BEACON_INTERVAL_MS: u64 = 1_000;

#[embassy_executor::task]
pub async fn espnow_rx_task(
    init: &'static EspWifiController<'static>,
    token: EspNowWithWifiCreateToken,
    // Add display intent channel when ready
) {
    log::info!("[espnow] task starting");

    let t0 = embassy_time::Instant::now();
    let mut esp_now = EspNow::new_with_wifi(init, token).unwrap();
    let dt = (embassy_time::Instant::now() - t0).as_millis();
    log::info!("[espnow] EspNow::new_with_wifi — done ({}ms)", dt);

    let mut beacon_deadline = embassy_time::Instant::now()
        + embassy_time::Duration::from_millis(BEACON_INTERVAL_MS);

    loop {
        match embassy_time::with_timeout(
            beacon_deadline.saturating_duration_since(embassy_time::Instant::now()),
            esp_now.receive_async(),
        )
        .await
        {
            Ok(frame) => {
                let now_ms = embassy_time::Instant::now().as_millis();
                RADIO_LAST_RX_MS.store(now_ms, Ordering::Release);

                // Attempt to parse JSON intent
                // Example payload: `{"emotion":"happy"}`
                // Since this is bare metal, we just do a crude string matching for proof of concept.
                if let Ok(s) = core::str::from_utf8(frame.data()) {
                    if s.contains("happy") {
                        // Dispatch happy intent
                        log::info!("Received intent: Happy");
                        let _ = crate::display::DISPLAY_CHANNEL.sender().try_send(crate::display::DisplayCommand::SetExpression(abi::EyeExpression::Happy));
                    } else if s.contains("sad") {
                        log::info!("Received intent: Sad");
                        let _ = crate::display::DISPLAY_CHANNEL.sender().try_send(crate::display::DisplayCommand::SetExpression(abi::EyeExpression::Sad));
                    } else if s.contains("neutral") {
                        log::info!("Received intent: Neutral");
                        let _ = crate::display::DISPLAY_CHANNEL.sender().try_send(crate::display::DisplayCommand::SetExpression(abi::EyeExpression::Neutral));
                    } else {
                        log::info!("Received intent: {:?}", s);
                    }
                }
            }
            Err(_timeout) => {
                beacon_deadline = embassy_time::Instant::now()
                    + embassy_time::Duration::from_millis(BEACON_INTERVAL_MS);
            }
        }
    }
}
