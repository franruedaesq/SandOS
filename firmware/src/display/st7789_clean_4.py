import re

with open("firmware/src/display/mod.rs", "r") as f:
    code = f.read()

code = code.replace("if crate::web_server::is_web_server_enabled() { 1 } else { 0 };", "0;")

# Make unused variables passed to functions use underscores
code = re.sub(r"fn render_web_menu_panel\(oled: &mut St7789Display, state: &FaceState\)", "fn render_web_menu_panel(oled: &mut St7789Display, _state: &FaceState)", code)
code = re.sub(r"fn render_vienna_lines\(oled: &mut St7789Display, state: &mut FaceState\)", "fn render_vienna_lines(oled: &mut St7789Display, _state: &mut FaceState)", code)
code = re.sub(r"fn render_vienna_detail\(oled: &mut St7789Display, state: &FaceState\)", "fn render_vienna_detail(_oled: &mut St7789Display, _state: &FaceState)", code)

with open("firmware/src/display/mod.rs", "w") as f:
    f.write(code)
