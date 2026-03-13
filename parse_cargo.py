import re

with open("firmware/Cargo.toml", "r") as f:
    content = f.read()

# Add dependency safely
if "ft6336u-driver" not in content:
    content = re.sub(r'(smart-leds = "0.3")', r'\1\nft6336u-driver = "1.0.0"\n', content)

with open("firmware/Cargo.toml", "w") as f:
    f.write(content)
