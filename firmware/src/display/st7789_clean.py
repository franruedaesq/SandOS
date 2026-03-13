import re

with open("firmware/src/display/mod.rs", "r") as f:
    code = f.read()

# Add `use embedded_hal_async::spi::SpiBus;` if not present
if "embedded_hal_async::spi::SpiBus" not in code:
    code = code.replace("use embedded_graphics::{", "use embedded_hal_async::spi::SpiBus;\nuse embedded_graphics::{")

# Replace `write_bytes(x).await` with `write(x).await` and also the synchronous `write_bytes(x)` we had back to `.write(x).await`
code = code.replace("self.spi.write_bytes(&[cmd])", "self.spi.write(&[cmd]).await")
code = code.replace("self.spi.write_bytes(data)", "self.spi.write(data).await")
code = code.replace("self.spi.write_bytes(chunk)", "self.spi.write(chunk).await")

with open("firmware/src/display/mod.rs", "w") as f:
    f.write(code)
