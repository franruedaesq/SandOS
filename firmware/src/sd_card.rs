use alloc::vec::Vec;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use embedded_sdmmc::{SdCard, TimeSource, Timestamp, VolumeIdx, VolumeManager};
use esp_hal::spi::master::Spi;
use esp_hal::Blocking;
use esp_hal::gpio::Output;
use embedded_hal_bus::spi::ExclusiveDevice;
use embedded_io::{Read, Seek};

pub struct DummyTimeSource;

impl TimeSource for DummyTimeSource {
    fn get_timestamp(&self) -> Timestamp {
        Timestamp::from_calendar(2023, 1, 1, 0, 0, 0).unwrap()
    }
}

pub type SdCardDevice<'a> = ExclusiveDevice<Spi<'a, Blocking>, Output<'a>, esp_hal::delay::Delay>;
pub type SdCardManager<'a> = VolumeManager<SdCard<SdCardDevice<'a>, esp_hal::delay::Delay>, DummyTimeSource, 1, 1, 1>;

pub static SD_CARD_MANAGER: Mutex<CriticalSectionRawMutex, Option<SdCardManager<'static>>> =
    Mutex::new(None);

pub fn init_sd_card(spi: Spi<'static, Blocking>, cs: Output<'static>) {
    let delay = esp_hal::delay::Delay::new();
    let spi_device = ExclusiveDevice::new(spi, cs, delay).unwrap();
    let sdcard = SdCard::new(spi_device, esp_hal::delay::Delay::new());
    let manager = VolumeManager::new_with_limits(sdcard, DummyTimeSource, 0);

    if let Ok(mut m) = SD_CARD_MANAGER.try_lock() {
        *m = Some(manager);
    }
}

pub async fn stream_asset_chunk(filename: &str, offset: u32, buf: &mut [u8]) -> usize {
    let mut manager_opt = SD_CARD_MANAGER.lock().await;

    if let Some(ref mut manager) = *manager_opt {
        if let Ok(volume) = manager.open_volume(VolumeIdx(0)) {
            if let Ok(root_dir) = volume.open_root_dir() {
                if let Ok(mut file) = root_dir.open_file_in_dir(filename, embedded_sdmmc::Mode::ReadOnly) {
                    if file.seek(embedded_io::SeekFrom::Start(offset as u64)).is_ok() {
                        if let Ok(bytes_read) = file.read(buf) {
                            let _ = file.close();
                            return bytes_read;
                        }
                    }
                    let _ = file.close();
                }
            }
        }
    }

    0
}

pub async fn read_wasm_file(filename: &str) -> Option<Vec<u8>> {
    let mut manager_opt = SD_CARD_MANAGER.lock().await;

    if let Some(ref mut manager) = *manager_opt {
        if let Ok(volume) = manager.open_volume(VolumeIdx(0)) {
            if let Ok(root_dir) = volume.open_root_dir() {
                if let Ok(mut file) = root_dir.open_file_in_dir(filename, embedded_sdmmc::Mode::ReadOnly) {
                    let mut contents = Vec::new();
                    let mut buffer = [0u8; 512];

                    while let Ok(bytes_read) = file.read(&mut buffer) {
                        if bytes_read == 0 {
                            break;
                        }
                        contents.extend_from_slice(&buffer[..bytes_read]);
                    }

                    let _ = file.close();
                    return Some(contents);
                }
            }
        }
    }

    None
}
