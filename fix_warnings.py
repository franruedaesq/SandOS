import os

with open("firmware/src/display/mod.rs", "r") as f:
    code = f.read()

code = code.replace("SetExpression(EyeExpression),", "SetExpression(EyeExpression),")
# Wait, let's just add #[allow(dead_code)] to mod.rs for now since those enum variants are part of the ABI channel
code = "#![allow(dead_code)]\n" + code

with open("firmware/src/display/mod.rs", "w") as f:
    f.write(code)

with open("firmware/src/core0/abi.rs", "r") as f:
    code = f.read()
code = "#![allow(dead_code)]\n" + code
with open("firmware/src/core0/abi.rs", "w") as f:
    f.write(code)

with open("firmware/src/message_bus.rs", "r") as f:
    code = f.read()
code = "#![allow(dead_code)]\n" + code
with open("firmware/src/message_bus.rs", "w") as f:
    f.write(code)

with open("firmware/src/ntp.rs", "r") as f:
    code = f.read()
code = "#![allow(dead_code)]\n" + code
with open("firmware/src/ntp.rs", "w") as f:
    f.write(code)

with open("firmware/src/ulp/mod.rs", "r") as f:
    code = f.read()
code = "#![allow(dead_code)]\n" + code
with open("firmware/src/ulp/mod.rs", "w") as f:
    f.write(code)

with open("firmware/src/vienna_fetch.rs", "r") as f:
    code = f.read()
code = "#![allow(dead_code)]\n" + code
with open("firmware/src/vienna_fetch.rs", "w") as f:
    f.write(code)

with open("firmware/src/wifi.rs", "r") as f:
    code = f.read()
code = "#![allow(dead_code)]\n" + code
with open("firmware/src/wifi.rs", "w") as f:
    f.write(code)
