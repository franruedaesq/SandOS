use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use embassy_time::{Duration, Timer};
use esp_hal::{
    gpio::{GpioPin, Input, Level, Output, Pull},
    i2c::master::{BusTimeout, Config as I2cConfig, I2c},
    peripherals::I2C1,
    time::RateExtU32,
    Async,
};

use crate::hardware_profile::{set_touch_state, ModuleState};

const FT6336_I2C_ADDR: u8 = 0x38;
const FT6336_REG_CHIP_ID: u8 = 0xA3;
const FT6336_REG_TOUCH_DATA: u8 = 0x02;
const FT6336_TOUCH_PACKET_LEN: usize = 11;
const FT6336_MAX_TOUCH_POINTS: u8 = 2;

const RAW_TOUCH_WIDTH: u16 = 240;
const RAW_TOUCH_HEIGHT: u16 = 320;
const DISPLAY_WIDTH: u16 = 128;
const DISPLAY_HEIGHT: u16 = 64;

const JITTER_THRESHOLD_PX: i16 = 2;

const EVENT_DOWN: u8 = 0;
const EVENT_UP: u8 = 1;
const EVENT_CONTACT: u8 = 2;

const TOUCH_EVENT_QUEUE_DEPTH: usize = 12;

static TOUCH_EVENT_CHANNEL: Channel<CriticalSectionRawMutex, TouchEvent, TOUCH_EVENT_QUEUE_DEPTH> =
    Channel::new();
static HOST_TOUCH_EVENT_CHANNEL: Channel<
    CriticalSectionRawMutex,
    TouchEvent,
    TOUCH_EVENT_QUEUE_DEPTH,
> = Channel::new();

#[derive(Clone, Copy, Debug)]
pub enum TouchPhase {
    Down,
    Move,
    Up,
}

#[derive(Clone, Copy, Debug)]
pub struct TouchEvent {
    pub phase: TouchPhase,
    pub x: u16,
    pub y: u16,
    pub id: u8,
}

#[derive(Clone, Copy)]
struct Contact {
    phase: TouchPhase,
    x: u16,
    y: u16,
    id: u8,
}

pub fn receiver(
) -> embassy_sync::channel::Receiver<'static, CriticalSectionRawMutex, TouchEvent, TOUCH_EVENT_QUEUE_DEPTH>
{
    TOUCH_EVENT_CHANNEL.receiver()
}

pub fn host_receiver(
) -> embassy_sync::channel::Receiver<'static, CriticalSectionRawMutex, TouchEvent, TOUCH_EVENT_QUEUE_DEPTH>
{
    HOST_TOUCH_EVENT_CHANNEL.receiver()
}

#[derive(Clone, Copy)]
enum TouchOrientation {
    Portrait,
}

fn normalize_coords(raw_x: u16, raw_y: u16) -> (u16, u16) {
    let (ori_x, ori_y) = match TouchOrientation::Portrait {
        TouchOrientation::Portrait => (raw_x, raw_y),
    };

    let x = (u32::from(ori_x) * u32::from(DISPLAY_WIDTH.saturating_sub(1)))
        / u32::from(RAW_TOUCH_WIDTH.saturating_sub(1));
    let y = (u32::from(ori_y) * u32::from(DISPLAY_HEIGHT.saturating_sub(1)))
        / u32::from(RAW_TOUCH_HEIGHT.saturating_sub(1));
    (x as u16, y as u16)
}

fn filter_contact(contact: Contact, last: Option<TouchEvent>) -> TouchEvent {
    let (mut x, mut y) = normalize_coords(contact.x, contact.y);

    if matches!(contact.phase, TouchPhase::Move) {
        if let Some(prev) = last {
            let dx = x as i16 - prev.x as i16;
            let dy = y as i16 - prev.y as i16;
            if dx.abs() <= JITTER_THRESHOLD_PX {
                x = prev.x;
            }
            if dy.abs() <= JITTER_THRESHOLD_PX {
                y = prev.y;
            }
        }
    }

    TouchEvent {
        phase: contact.phase,
        x,
        y,
        id: contact.id,
    }
}

fn parse_contact(bytes: &[u8]) -> Option<Contact> {
    if bytes.len() < 6 {
        return None;
    }

    let event_flag = (bytes[0] >> 6) & 0x03;
    let x = (((bytes[0] & 0x0f) as u16) << 8) | bytes[1] as u16;
    let id = (bytes[2] >> 4) & 0x0f;
    let y = (((bytes[2] & 0x0f) as u16) << 8) | bytes[3] as u16;

    let phase = match event_flag {
        EVENT_DOWN => TouchPhase::Down,
        EVENT_UP => TouchPhase::Up,
        EVENT_CONTACT => TouchPhase::Move,
        _ => return None,
    };

    Some(Contact { phase, x, y, id })
}

#[embassy_executor::task]
pub async fn event_task(
    i2c1: I2C1,
    sda: GpioPin<16>,
    scl: GpioPin<15>,
    touch_rst: GpioPin<18>,
    touch_int: GpioPin<17>,
) {
    let mut cfg = I2cConfig::default();
    // Shared with external I2C peripheral header: keep conservative clock for signal integrity.
    cfg.frequency = 200.kHz();
    cfg.timeout = BusTimeout::BusCycles(80_000);

    let mut i2c = match I2c::new(i2c1, cfg) {
        Ok(bus) => bus.with_sda(sda).with_scl(scl).into_async(),
        Err(err) => {
            log::error!("[touch] I2C1 init failed: {:?}", err);
            set_touch_state(ModuleState::Fault);
            return;
        }
    };

    let mut rst = Output::new(touch_rst, Level::High);
    rst.set_low();
    Timer::after(Duration::from_millis(8)).await;
    rst.set_high();
    Timer::after(Duration::from_millis(160)).await;

    let mut int_pin = Input::new(touch_int, Pull::Up);

    let mut chip_id = [0u8; 1];
    if i2c
        .write_read(FT6336_I2C_ADDR, &[FT6336_REG_CHIP_ID], &mut chip_id)
        .await
        .is_err()
    {
        log::error!("[touch] FT6336 chip id probe failed");
        set_touch_state(ModuleState::Fault);
        return;
    }

    if chip_id[0] == 0x00 || chip_id[0] == 0xFF {
        log::error!("[touch] FT6336 probe returned invalid chip id=0x{:02X}", chip_id[0]);
        set_touch_state(ModuleState::Fault);
        return;
    }

    set_touch_state(ModuleState::Configured);
    log::info!("[touch] FT6336 initialized chip_id=0x{:02X}", chip_id[0]);

    let mut seen_valid_read_path = false;
    let mut packet = [0u8; FT6336_TOUCH_PACKET_LEN];
    let mut last_points: [Option<TouchEvent>; FT6336_MAX_TOUCH_POINTS as usize] = [None, None];

    loop {
        int_pin.wait_for_falling_edge().await;

        match i2c
            .write_read(FT6336_I2C_ADDR, &[FT6336_REG_TOUCH_DATA], &mut packet)
            .await
        {
            Ok(()) => {
                let count = packet[0] & 0x0f;
                if count > FT6336_MAX_TOUCH_POINTS {
                    continue;
                }

                if !seen_valid_read_path {
                    seen_valid_read_path = true;
                    set_touch_state(ModuleState::Online);
                    log::info!("[touch] event path online");
                }

                if count >= 1 {
                    if let Some(contact) = parse_contact(&packet[1..7]) {
                        let id = (contact.id as usize) % last_points.len();
                        let event = filter_contact(contact, last_points[id]);
                        last_points[id] = if matches!(event.phase, TouchPhase::Up) {
                            None
                        } else {
                            Some(event)
                        };
                        let _ = TOUCH_EVENT_CHANNEL.sender().try_send(event);
                        let _ = HOST_TOUCH_EVENT_CHANNEL.sender().try_send(event);
                    }
                }

                if count >= 2 {
                    if let Some(contact) = parse_contact(&packet[7..]) {
                        let id = (contact.id as usize) % last_points.len();
                        let event = filter_contact(contact, last_points[id]);
                        last_points[id] = if matches!(event.phase, TouchPhase::Up) {
                            None
                        } else {
                            Some(event)
                        };
                        let _ = TOUCH_EVENT_CHANNEL.sender().try_send(event);
                        let _ = HOST_TOUCH_EVENT_CHANNEL.sender().try_send(event);
                    }
                }
            }
            Err(err) => {
                set_touch_state(ModuleState::Fault);
                log::warn!("[touch] FT6336 event read failed: {:?}", err);
            }
        }
    }
}
