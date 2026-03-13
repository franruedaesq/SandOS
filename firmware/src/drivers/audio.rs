use core::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex,
    channel::{Channel, TrySendError},
};
use esp_hal::gpio::{GpioPin, Output};

use crate::hardware_profile::{set_audio_state, ModuleState, AUDIO_EN};

const DMA_FRAME_BYTES: usize = 256;
const DMA_RING_BUFFERS: usize = 2;
const CAPTURE_QUEUE_DEPTH: usize = 4;

/// Raw PCM frame produced by the microphone RX pipeline.
pub type AudioDmaFrame = heapless::Vec<u8, DMA_FRAME_BYTES>;

/// Lock-free channel carrying captured DMA frames to the ABI host.
pub static AUDIO_CAPTURE_CHANNEL: Channel<
    CriticalSectionRawMutex,
    AudioDmaFrame,
    CAPTURE_QUEUE_DEPTH,
> = Channel::new();

/// Total number of DMA frames dropped due to backpressure.
static AUDIO_OVERRUN_FRAMES: AtomicU32 = AtomicU32::new(0);
/// Total number of bytes dropped from the channel producer side.
static AUDIO_DROPPED_BYTES: AtomicUsize = AtomicUsize::new(0);
/// Capture gate controlled by ABI `start_audio_capture()` / `stop_audio_capture()`.
static AUDIO_CAPTURE_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Enable the microphone RX pipeline.
#[inline]
pub fn start_capture() {
    AUDIO_CAPTURE_ACTIVE.store(true, Ordering::Release);
}

/// Disable the microphone RX pipeline.
#[inline]
pub fn stop_capture() {
    AUDIO_CAPTURE_ACTIVE.store(false, Ordering::Release);
}

/// Returns whether capture is currently enabled.
#[inline]
pub fn is_capture_active() -> bool {
    AUDIO_CAPTURE_ACTIVE.load(Ordering::Acquire)
}

/// Snapshot `(overrun_frames, dropped_bytes)` from the DMA producer.
#[inline]
pub fn capture_stats() -> (u32, usize) {
    (
        AUDIO_OVERRUN_FRAMES.load(Ordering::Relaxed),
        AUDIO_DROPPED_BYTES.load(Ordering::Relaxed),
    )
}

fn push_dma_frame(frame: AudioDmaFrame) {
    match AUDIO_CAPTURE_CHANNEL.try_send(frame) {
        Ok(()) => {}
        Err(TrySendError::Full(frame)) => {
            AUDIO_OVERRUN_FRAMES.fetch_add(1, Ordering::Relaxed);
            AUDIO_DROPPED_BYTES.fetch_add(frame.len(), Ordering::Relaxed);
        }
        Err(TrySendError::Closed(_)) => {}
    }
}

fn synthesize_i2s_frame(seq: &mut u8, out: &mut [u8; DMA_FRAME_BYTES]) {
    // Placeholder for real I2S RX DMA reads. The pipeline is wired exactly as
    // production code expects; only this source function is swapped for the HAL
    // I2S DMA reader once hardware validation is available.
    for b in out.iter_mut() {
        *b = *seq;
        *seq = seq.wrapping_add(1);
    }
}

#[embassy_executor::task]
pub async fn probe_task(audio_en: GpioPin<1>) {
    let mut amp_en = Output::new(audio_en, esp_hal::gpio::Level::Low);
    amp_en.set_high();
    set_audio_state(ModuleState::Online);
    log::info!(
        "[audio] RX pipeline ready (DMA ring={}x{}B)",
        DMA_RING_BUFFERS,
        DMA_FRAME_BYTES
    );

    let mut dma_ring = [[0u8; DMA_FRAME_BYTES]; DMA_RING_BUFFERS];
    let mut ring_index = 0usize;
    let mut seq: u8 = 0;

    loop {
        if !is_capture_active() {
            embassy_time::Timer::after(embassy_time::Duration::from_millis(10)).await;
            continue;
        }

        let buf = &mut dma_ring[ring_index];
        ring_index = (ring_index + 1) % DMA_RING_BUFFERS;

        synthesize_i2s_frame(&mut seq, buf);

        let mut frame = AudioDmaFrame::new();
        frame.extend_from_slice(buf).ok();
        push_dma_frame(frame);

        amp_en.set_high();
        embassy_time::Timer::after(embassy_time::Duration::from_millis(8)).await;
    }
}
