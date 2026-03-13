use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use esp_hal::gpio::{GpioPin, Output, Level};
use esp_hal::i2s::master::{I2s, Standard, I2sRx, I2sTx, DataFormat};
use esp_hal::dma::DmaDescriptor;
use esp_hal::dma::DmaChannelFor;
use esp_hal::peripherals::I2S0;
use esp_hal::time::RateExtU32;
use esp_hal::Async;

static mut RX_DESC: [DmaDescriptor; 1] = [DmaDescriptor::EMPTY; 1];
static mut TX_DESC: [DmaDescriptor; 1] = [DmaDescriptor::EMPTY; 1];

pub fn spawn_audio_tasks<CH: DmaChannelFor<esp_hal::i2s::master::AnyI2s> + 'static>(
    spawner: Spawner,
    i2s0: I2S0,
    mclk: GpioPin<4>,
    bclk: GpioPin<5>,
    ws: GpioPin<7>,
    dout: GpioPin<6>,
    din: GpioPin<8>,
    amp_en: GpioPin<1>,
    dma_channel: CH,
) {
    // Keep amp_en alive by converting it into a leaked static or static mutable.
    // In our case we just need it low to enable. We can bypass dropping by using `core::mem::forget`
    // or properly managing its lifetime.
    let mut amp_enable = Output::new(amp_en, Level::Low);
    amp_enable.set_low();
    // Prevent the output from being dropped, keeping the pin configured.
    core::mem::forget(amp_enable);

    let standard = Standard::Philips;

    let rx_descriptors = unsafe { &mut *core::ptr::addr_of_mut!(RX_DESC) };
    let tx_descriptors = unsafe { &mut *core::ptr::addr_of_mut!(TX_DESC) };

    let i2s = I2s::new(
        i2s0,
        standard,
        DataFormat::Data16Channel16,
        16000u32.Hz(),
        dma_channel,
        rx_descriptors,
        tx_descriptors,
    );

    let i2s = i2s.with_mclk(mclk).into_async();

    // In current esp-hal, pins are assigned to tx/rx sub-components
    let tx = i2s.i2s_tx.with_bclk(bclk).with_ws(ws).with_dout(dout).build();
    let rx = i2s.i2s_rx.with_din(din).build();

    // Spawn placeholder tasks
    spawner.spawn(mic_rx_task(rx)).unwrap();
    spawner.spawn(speaker_tx_task(tx)).unwrap();
}

#[embassy_executor::task]
async fn mic_rx_task(rx: I2sRx<'static, Async>) {
    log::info!("[audio] Mic RX task started");
    let mut rx_buf = [0u8; 4096];

    let mut transfer = rx.read_dma_circular_async(&mut rx_buf).unwrap();

    loop {
        // Pop available bytes asynchronously without blocking the executor
        if let Ok(avail) = transfer.available().await {
            if avail > 0 {
                let mut chunk = [0u8; 1024];
                let read_len = core::cmp::min(avail, chunk.len());
                let _ = transfer.pop(&mut chunk[..read_len]).await;

                // In a real implementation we would route this audio to inference or wifi
            }
        }
        Timer::after(Duration::from_millis(50)).await;
    }
}

#[embassy_executor::task]
async fn speaker_tx_task(tx: I2sTx<'static, Async>) {
    log::info!("[audio] Speaker TX task started");
    let mut tx_buf = [0u8; 4096];

    let mut transfer = tx.write_dma_circular_async(&mut tx_buf).unwrap();

    loop {
        // Push bytes asynchronously without blocking the executor
        if let Ok(avail) = transfer.available().await {
            if avail > 0 {
                let dummy_chunk = [0u8; 1024];
                let write_len = core::cmp::min(avail, dummy_chunk.len());
                let _ = transfer.push(&dummy_chunk[..write_len]).await;
            }
        }

        Timer::after(Duration::from_millis(50)).await;
    }
}
