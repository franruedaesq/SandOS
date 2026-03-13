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

const EVENT_DOWN: u8 = 0;
const EVENT_UP: u8 = 1;
const EVENT_CONTACT: u8 = 2;

const TOUCH_EVENT_QUEUE_DEPTH: usize = 12;

static TOUCH_EVENT_CHANNEL: Channel<CriticalSectionRawMutex, TouchEvent, TOUCH_EVENT_QUEUE_DEPTH> =
    Channel::new();

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
    cfg.frequency = 400.kHz();
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

    set_touch_state(ModuleState::Configured);
    log::info!("[touch] FT6336 initialized chip_id=0x{:02X}", chip_id[0]);

    let mut seen_valid_read_path = false;
    let mut packet = [0u8; FT6336_TOUCH_PACKET_LEN];

    loop {
        int_pin.wait_for_falling_edge().await;

        match i2c
            .write_read(FT6336_I2C_ADDR, &[FT6336_REG_TOUCH_DATA], &mut packet)
            .await
        {
            Ok(()) => {
                let count = packet[0] & 0x0f;
                if count > 2 {
                    continue;
                }

                if !seen_valid_read_path {
                    seen_valid_read_path = true;
                    set_touch_state(ModuleState::Online);
                    log::info!("[touch] event path online");
                }

                if count >= 1 {
                    if let Some(contact) = parse_contact(&packet[1..7]) {
                        let _ = TOUCH_EVENT_CHANNEL.sender().try_send(TouchEvent {
                            phase: contact.phase,
                            x: contact.x,
                            y: contact.y,
                            id: contact.id,
                        });
                    }
                }

                if count >= 2 {
                    if let Some(contact) = parse_contact(&packet[7..]) {
                        let _ = TOUCH_EVENT_CHANNEL.sender().try_send(TouchEvent {
                            phase: contact.phase,
                            x: contact.x,
                            y: contact.y,
                            id: contact.id,
                        });
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
