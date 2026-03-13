use core::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex,
    channel::{Channel, TrySendError},
};
use esp_hal::gpio::{GpioPin, Output};

use crate::hardware_profile::{set_audio_state, ModuleState};

const DMA_FRAME_BYTES: usize = 256;
const DMA_RING_BUFFERS: usize = 2;
const CAPTURE_QUEUE_DEPTH: usize = 4;
const PLAYBACK_QUEUE_DEPTH: usize = 8;
const AUDIO_PROFILE_SAMPLE_RATE_HZ: u32 = 16_000;
const AUDIO_PROFILE_BITS_PER_SAMPLE: u8 = 16;
const AUDIO_PROFILE_CHANNELS: u8 = 1;
const PLAYBACK_SAMPLE_RATE_HZ: u32 = 16_000;
const PLAYBACK_BITS_PER_SAMPLE: u8 = 16;
const PLAYBACK_CHANNELS: u8 = 1;
const RAMP_STEPS: u16 = 16;
const VAD_ABS_THRESHOLD: i16 = 180;

/// Raw PCM frame consumed by the speaker TX pipeline.
pub type AudioPlaybackFrame = heapless::Vec<u8, DMA_FRAME_BYTES>;

/// Raw PCM frame produced by the microphone RX pipeline.
pub type AudioDmaFrame = heapless::Vec<u8, DMA_FRAME_BYTES>;

/// Lock-free channel carrying captured DMA frames to the ABI host.
pub static AUDIO_CAPTURE_CHANNEL: Channel<
    CriticalSectionRawMutex,
    AudioDmaFrame,
    CAPTURE_QUEUE_DEPTH,
> = Channel::new();

pub static AUDIO_PLAYBACK_PCM_CHANNEL: Channel<
    CriticalSectionRawMutex,
    AudioPlaybackFrame,
    PLAYBACK_QUEUE_DEPTH,
> = Channel::new();

/// Total number of DMA frames dropped due to backpressure.
static AUDIO_OVERRUN_FRAMES: AtomicU32 = AtomicU32::new(0);
/// Total number of bytes dropped from the channel producer side.
static AUDIO_DROPPED_BYTES: AtomicUsize = AtomicUsize::new(0);
/// Capture gate controlled by ABI `start_audio_capture()` / `stop_audio_capture()`.
static AUDIO_CAPTURE_ACTIVE: AtomicBool = AtomicBool::new(false);
/// Playback gate inferred from pending stream and tone requests.
static AUDIO_PLAYBACK_ACTIVE: AtomicBool = AtomicBool::new(false);
static AUDIO_PLAYBACK_OVERRUN_FRAMES: AtomicU32 = AtomicU32::new(0);
static AUDIO_PLAYBACK_UNDERRUN_FRAMES: AtomicU32 = AtomicU32::new(0);
static AUDIO_VAD_ENABLED: AtomicBool = AtomicBool::new(false);
static AUDIO_VAD_DROPPED_FRAMES: AtomicU32 = AtomicU32::new(0);

#[derive(Clone, Copy)]
pub struct ToneRequest {
    pub freq_hz: u16,
    pub duration_ms: u32,
    pub amplitude_pct: u8,
}

pub enum AudioOutCommand {
    Tone(ToneRequest),
    Stop,
    SetLoopbackGain(u8),
}

pub static AUDIO_PLAYBACK_COMMAND_CHANNEL: Channel<
    CriticalSectionRawMutex,
    AudioOutCommand,
    PLAYBACK_QUEUE_DEPTH,
> = Channel::new();

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

#[inline]
pub fn is_playback_active() -> bool {
    AUDIO_PLAYBACK_ACTIVE.load(Ordering::Acquire)
}

pub fn queue_pcm_chunk(pcm: &[u8]) -> usize {
    let mut queued = 0usize;
    for chunk in pcm.chunks(DMA_FRAME_BYTES) {
        let mut frame = AudioPlaybackFrame::new();
        let _ = frame.extend_from_slice(chunk);
        match AUDIO_PLAYBACK_PCM_CHANNEL.try_send(frame) {
            Ok(()) => queued += chunk.len(),
            Err(TrySendError::Full(_)) => {
                AUDIO_PLAYBACK_OVERRUN_FRAMES.fetch_add(1, Ordering::Relaxed);
                break;
            }
        }
    }

    if queued > 0 {
        AUDIO_PLAYBACK_ACTIVE.store(true, Ordering::Release);
    }
    queued
}

pub fn playback_stats() -> (u32, u32) {
    (
        AUDIO_PLAYBACK_OVERRUN_FRAMES.load(Ordering::Relaxed),
        AUDIO_PLAYBACK_UNDERRUN_FRAMES.load(Ordering::Relaxed),
    )
}

pub fn set_vad_enabled(enabled: bool) {
    AUDIO_VAD_ENABLED.store(enabled, Ordering::Release);
}

pub fn vad_stats() -> u32 {
    AUDIO_VAD_DROPPED_FRAMES.load(Ordering::Relaxed)
}

pub fn queue_tone(req: ToneRequest) -> Result<(), TrySendError<AudioOutCommand>> {
    AUDIO_PLAYBACK_COMMAND_CHANNEL.try_send(AudioOutCommand::Tone(req))?;
    AUDIO_PLAYBACK_ACTIVE.store(true, Ordering::Release);
    Ok(())
}

pub fn set_loopback_gain(gain_pct: u8) -> Result<(), TrySendError<AudioOutCommand>> {
    AUDIO_PLAYBACK_COMMAND_CHANNEL.try_send(AudioOutCommand::SetLoopbackGain(gain_pct.min(100)))
}

pub fn stop_playback() {
    let _ = AUDIO_PLAYBACK_COMMAND_CHANNEL.try_send(AudioOutCommand::Stop);
}

fn push_dma_frame(frame: AudioDmaFrame) {
    match AUDIO_CAPTURE_CHANNEL.try_send(frame) {
        Ok(()) => {}
        Err(TrySendError::Full(frame)) => {
            AUDIO_OVERRUN_FRAMES.fetch_add(1, Ordering::Relaxed);
            AUDIO_DROPPED_BYTES.fetch_add(frame.len(), Ordering::Relaxed);
        }
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

fn frame_has_voice_activity(frame: &[u8; DMA_FRAME_BYTES]) -> bool {
    let mut i = 0usize;
    while i + 1 < frame.len() {
        let s = i16::from_le_bytes([frame[i], frame[i + 1]]).abs();
        if s >= VAD_ABS_THRESHOLD {
            return true;
        }
        i += 2;
    }
    false
}

fn ramp_sample(sample: i16, step: u16) -> i16 {
    ((sample as i32 * step as i32) / RAMP_STEPS as i32) as i16
}

fn apply_pop_ramp(buf: &mut [u8; DMA_FRAME_BYTES], ramp_step: u16) {
    if ramp_step >= RAMP_STEPS {
        return;
    }

    let mut i = 0usize;
    while i + 1 < buf.len() {
        let s = i16::from_le_bytes([buf[i], buf[i + 1]]);
        let r = ramp_sample(s, ramp_step);
        let bytes = r.to_le_bytes();
        buf[i] = bytes[0];
        buf[i + 1] = bytes[1];
        i += 2;
    }
}

fn synthesize_tone_frame(phase: &mut u32, req: ToneRequest, out: &mut [u8; DMA_FRAME_BYTES]) {
    let amplitude = ((i16::MAX as i32) * req.amplitude_pct as i32 / 100) as i16;
    let phase_inc = ((req.freq_hz as u32) << 16) / PLAYBACK_SAMPLE_RATE_HZ.max(1);

    let mut i = 0usize;
    while i + 1 < out.len() {
        let saw = ((*phase >> 8) & 0xFF) as i16 - 128;
        let sample = ((saw as i32 * amplitude as i32) / 128) as i16;
        let bytes = sample.to_le_bytes();
        out[i] = bytes[0];
        out[i + 1] = bytes[1];
        *phase = phase.wrapping_add(phase_inc);
        i += 2;
    }
}

fn scale_pcm_in_place(buf: &mut AudioPlaybackFrame, gain_pct: u8) {
    let gain = gain_pct.min(100) as i32;
    let bytes: &mut [u8] = buf.as_mut_slice();
    let mut i = 0usize;
    while i + 1 < bytes.len() {
        let s = i16::from_le_bytes([bytes[i], bytes[i + 1]]);
        let scaled = ((s as i32 * gain) / 100) as i16;
        let out = scaled.to_le_bytes();
        bytes[i] = out[0];
        bytes[i + 1] = out[1];
        i += 2;
    }
}

#[embassy_executor::task]
pub async fn probe_task(audio_en: GpioPin<1>) {
    let mut amp_en = Output::new(audio_en, esp_hal::gpio::Level::Low);
    amp_en.set_low();
    set_audio_state(ModuleState::Online);
    log::info!(
        "[audio] RX+TX pipeline ready (capture={}b/{}ch/{}Hz, playback={}b/{}ch/{}Hz, DMA ring={}x{}B)",
        AUDIO_PROFILE_BITS_PER_SAMPLE,
        AUDIO_PROFILE_CHANNELS,
        AUDIO_PROFILE_SAMPLE_RATE_HZ,
        PLAYBACK_BITS_PER_SAMPLE,
        PLAYBACK_CHANNELS,
        PLAYBACK_SAMPLE_RATE_HZ,
        DMA_RING_BUFFERS,
        DMA_FRAME_BYTES
    );

    let mut dma_ring = [[0u8; DMA_FRAME_BYTES]; DMA_RING_BUFFERS];
    let mut tx_ring = [[0u8; DMA_FRAME_BYTES]; DMA_RING_BUFFERS];
    let mut ring_index = 0usize;
    let mut seq: u8 = 0;
    let mut tone: Option<ToneRequest> = None;
    let mut tone_frames_remaining = 0u32;
    let mut tone_phase = 0u32;
    let mut loopback_gain_pct = 0u8;
    let mut amp_on = false;
    let mut ramp_step = 0u16;

    loop {
        while let Ok(cmd) = AUDIO_PLAYBACK_COMMAND_CHANNEL.try_receive() {
            match cmd {
                AudioOutCommand::Tone(req) => {
                    tone_frames_remaining =
                        req.duration_ms.saturating_mul(PLAYBACK_SAMPLE_RATE_HZ) / 1000
                            / (DMA_FRAME_BYTES as u32 / 2);
                    tone = Some(req);
                }
                AudioOutCommand::Stop => {
                    tone = None;
                    tone_frames_remaining = 0;
                }
                AudioOutCommand::SetLoopbackGain(gain) => loopback_gain_pct = gain,
            }
        }

        if !is_capture_active() {
            embassy_time::Timer::after(embassy_time::Duration::from_millis(10)).await;
        } else {
            let buf = &mut dma_ring[ring_index];
            synthesize_i2s_frame(&mut seq, buf);

            if AUDIO_VAD_ENABLED.load(Ordering::Acquire) && !frame_has_voice_activity(buf) {
                AUDIO_VAD_DROPPED_FRAMES.fetch_add(1, Ordering::Relaxed);
            } else {
                let mut frame = AudioDmaFrame::new();
                frame.extend_from_slice(buf).ok();
                push_dma_frame(frame.clone());

                if loopback_gain_pct > 0 {
                    let mut lb = AudioPlaybackFrame::new();
                    let _ = lb.extend_from_slice(frame.as_slice());
                    scale_pcm_in_place(&mut lb, loopback_gain_pct);
                    let _ = AUDIO_PLAYBACK_PCM_CHANNEL.try_send(lb);
                }
            }
        }

        let tx = &mut tx_ring[ring_index];
        tx.fill(0);
        ring_index = (ring_index + 1) % DMA_RING_BUFFERS;

        let mut stream_active = false;
        if let Ok(frame) = AUDIO_PLAYBACK_PCM_CHANNEL.try_receive() {
            tx[..frame.len()].copy_from_slice(frame.as_slice());
            stream_active = true;
        } else if tone.is_none() {
            AUDIO_PLAYBACK_UNDERRUN_FRAMES.fetch_add(1, Ordering::Relaxed);
        }

        if let Some(req) = tone {
            if tone_frames_remaining > 0 {
                synthesize_tone_frame(&mut tone_phase, req, tx);
                tone_frames_remaining = tone_frames_remaining.saturating_sub(1);
                stream_active = true;
            } else {
                tone = None;
            }
        }

        if stream_active && !amp_on {
            amp_on = true;
            amp_en.set_low();
            ramp_step = 0;
        }

        if amp_on {
            if stream_active {
                if ramp_step < RAMP_STEPS {
                    ramp_step += 1;
                }
                apply_pop_ramp(tx, ramp_step);
            } else if ramp_step > 0 {
                apply_pop_ramp(tx, ramp_step);
                ramp_step -= 1;
            } else {
                amp_en.set_high();
                amp_on = false;
            }
        }

        AUDIO_PLAYBACK_ACTIVE.store(stream_active || amp_on, Ordering::Release);
        embassy_time::Timer::after(embassy_time::Duration::from_millis(8)).await;
    }
}
