use esp_hal::i2s::master::{I2sRx, I2sTx};
use esp_hal::Async;
use embassy_sync::channel::Channel;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_time::{Duration, Timer};
use heapless::Vec;

use crate::router::{AUDIO_INFERENCE_CHANNEL, AudioSnapshot};

pub static AUDIO_RX_CHANNEL: Channel<CriticalSectionRawMutex, Vec<u8, 2048>, 4> = Channel::new();
pub static AUDIO_TX_CHANNEL: Channel<CriticalSectionRawMutex, Vec<u8, 2048>, 4> = Channel::new();

#[embassy_executor::task]
pub async fn audio_rx_task(i2s_rx: I2sRx<'static, Async>, rx_buffer: &'static mut [u8]) {
    if let Ok(mut transfer) = i2s_rx.read_dma_circular_async(rx_buffer) {
        let mut snapshot = AudioSnapshot::new();
        loop {
            if let Ok(avail) = transfer.available().await {
                if avail > 0 {
                    let mut buf = [0u8; 128];
                    let to_read = avail.min(buf.len());
                    if let Ok(read) = transfer.pop(&mut buf[..to_read]).await {
                        let mut chunk: Vec<u8, 2048> = Vec::new();
                        for i in 0..read {
                            if snapshot.is_full() {
                                let _ = AUDIO_INFERENCE_CHANNEL.try_send(snapshot.clone());
                                snapshot.clear();
                            }
                            // push as i8 (rough cast)
                            let _ = snapshot.push(buf[i] as i8);
                            let _ = chunk.push(buf[i]);
                        }
                        if !chunk.is_empty() {
                            let _ = AUDIO_RX_CHANNEL.try_send(chunk);
                        }
                    }
                }
            }
            Timer::after(Duration::from_millis(10)).await;
        }
    }
}

#[embassy_executor::task]
pub async fn audio_tx_task(i2s_tx: I2sTx<'static, Async>, tx_buffer: &'static mut [u8]) {
    if let Ok(mut transfer) = i2s_tx.write_dma_circular_async(tx_buffer) {
        loop {
            if let Ok(chunk) = AUDIO_TX_CHANNEL.try_receive() {
                let mut written = 0;
                while written < chunk.len() {
                    if let Ok(avail) = transfer.available().await {
                        let to_write = avail.min(chunk.len() - written);
                        if to_write > 0 {
                            if let Ok(wrote) = transfer.push(&chunk[written..written + to_write]).await {
                                written += wrote;
                            }
                        }
                    } else {
                        break;
                    }
                }
            }
            Timer::after(Duration::from_millis(5)).await;
        }
    }
}
