import re

with open("firmware/src/display/mod.rs", "r") as f:
    code = f.read()

# Fix web_server logic in frame rendering
code = re.sub(r"&& crate::web_server::is_web_server_enabled\(\)", "&& false", code)
code = re.sub(r"&& crate::wifi::wifi_status\(\) == crate::wifi::WIFI_STATUS_CONNECTING", "&& false", code)
code = re.sub(r"let web_enabled = if crate::web_server::is_web_server_enabled\(\) \{ 1 \} else \{ 0 \};", "let web_enabled = 0;", code)
code = re.sub(r"let data = crate::vienna_fetch::get_lines\(\);", "let data = core::iter::empty::<()>();", code)
code = re.sub(r"crate::web_server::enable_web_server\(\);", "", code)
code = re.sub(r"crate::web_server::disable_web_server\(\);", "", code)
code = re.sub(r"crate::wifi::mark_connecting\(\);", "", code)

with open("firmware/src/display/mod.rs", "w") as f:
    f.write(code)
