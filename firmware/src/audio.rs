//! Audio pipeline for I2S microphone capture and speaker playback.

use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex,
    channel::Channel,
};
use esp_hal::{
    i2s::master::{I2sRx, I2sTx},
    Async,
};
// ── Audio Shared Channels ───────────────────────────────────────────────────

/// Maximum size of an audio chunk sent to/from the ABI.
pub const AUDIO_CHUNK_SIZE: usize = 1024;

pub type AudioChunk = heapless::Vec<u8, AUDIO_CHUNK_SIZE>;

/// Channel to send captured audio chunks from the I2S DMA task to the Wasm ABI.
pub static AUDIO_RX_CHANNEL: Channel<CriticalSectionRawMutex, AudioChunk, 4> = Channel::new();

/// Channel to send audio chunks from the Wasm ABI to the I2S DMA playback task.
pub static AUDIO_TX_CHANNEL: Channel<CriticalSectionRawMutex, AudioChunk, 4> = Channel::new();

// ── Background Audio Tasks ──────────────────────────────────────────────────

#[embassy_executor::task]
pub async fn audio_rx_task(i2s_rx: I2sRx<'static, Async>, mut rx_buffer: &'static mut [u8]) {
    let mut transfer = i2s_rx.read_dma_circular_async(rx_buffer).unwrap();
    let mut chunk = AudioChunk::new();

    loop {
        let avail = transfer.available().await.unwrap_or(0);
        if avail > 0 {
            let mut rcv = [0u8; 1024];
            let read_len = avail.min(rcv.len());
            let popped = transfer.pop(&mut rcv[..read_len]).await.unwrap_or(0);

            for &byte in &rcv[..popped] {
                if chunk.push(byte).is_err() {
                    // Chunk is full, send it and start a new one
                    let _ = AUDIO_RX_CHANNEL.try_send(chunk.clone());
                    chunk.clear();
                    let _ = chunk.push(byte);
                }
            }
        } else {
            embassy_time::Timer::after_millis(2).await;
        }
    }
}

#[embassy_executor::task]
pub async fn audio_tx_task(i2s_tx: I2sTx<'static, Async>, mut tx_buffer: &'static mut [u8]) {
    let mut transfer = i2s_tx.write_dma_circular_async(tx_buffer).unwrap();

    loop {
        // Wait for an audio chunk from the ABI
        let chunk = AUDIO_TX_CHANNEL.receive().await;

        let mut offset = 0;
        while offset < chunk.len() {
            let space = transfer.available().await.unwrap_or(0);
            if space > 0 {
                let write_len = space.min(chunk.len() - offset);
                let pushed = transfer.push(&chunk[offset..offset + write_len]).await.unwrap_or(0);
                offset += pushed;
            } else {
                embassy_time::Timer::after_millis(2).await;
            }
        }
    }
}

pub fn play_blip() {
    let mut chunk = AudioChunk::new();
    for i in 0..64 {
        let val = if i % 2 == 0 { 0x3F } else { 0x00 };
        let _ = chunk.push(val);
    }
    let _ = AUDIO_TX_CHANNEL.try_send(chunk);
}
