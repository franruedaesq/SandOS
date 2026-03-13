use esp_hal::{
    i2c::master::I2c,
    Async,
};

const FT6336U_ADDR: u8 = 0x38;
const FT6336U_ADDR_TD_STATUS: u8 = 0x02;
const FT6336U_ADDR_TOUCH1_X: u8 = 0x03;
const FT6336U_ADDR_TOUCH1_Y: u8 = 0x05;

pub struct Ft6336<'a> {
    i2c: I2c<'a, Async>,
}

impl<'a> Ft6336<'a> {
    pub fn new(i2c: I2c<'a, Async>) -> Self {
        Self { i2c }
    }

    pub async fn read_touch(&mut self) -> Result<Option<(u16, u16)>, ()> {
        let mut td_status = [0u8; 1];
        if self.i2c.write_read(FT6336U_ADDR, &[FT6336U_ADDR_TD_STATUS], &mut td_status).await.is_err() {
            return Err(());
        }

        let touch_count = td_status[0] & 0x0F;
        if touch_count == 0 {
            return Ok(None);
        }

        let mut buf_x = [0u8; 2];
        if self.i2c.write_read(FT6336U_ADDR, &[FT6336U_ADDR_TOUCH1_X], &mut buf_x).await.is_err() {
            return Err(());
        }

        let mut buf_y = [0u8; 2];
        if self.i2c.write_read(FT6336U_ADDR, &[FT6336U_ADDR_TOUCH1_Y], &mut buf_y).await.is_err() {
            return Err(());
        }

        let x = (((buf_x[0] & 0x0F) as u16) << 8) | (buf_x[1] as u16);
        let y = (((buf_y[0] & 0x0F) as u16) << 8) | (buf_y[1] as u16);

        Ok(Some((x, y)))
    }
}
