import re

with open("firmware/src/display/mod.rs", "r") as f:
    code = f.read()

# Clean out the old unused items
code = code.replace("let web_enabled = if crate::web_server::is_web_server_enabled() { 1 } else { 0 };", "let web_enabled = 0;")

# Delete all contents inside the button action for menu 3 (which was vienna lines, etc)
# Just rip out the inner body of the vienna stops action
pattern = re.compile(r"if state\.current_menu_idx == 4 \{.*?let data = core::iter::empty::<.*?if !data\.stops\.is_empty\(\) \{.*?\}\n\s+\}", re.DOTALL)
code = pattern.sub("if state.current_menu_idx == 4 {\n}", code)

# Look for other vienna stops actions and nuke them
code = re.sub(r"let data = core::iter::empty::<.*?if !data\.stops\.is_empty\(\) \{.*?\}\n\s+", "", code, flags=re.DOTALL)

with open("firmware/src/display/mod.rs", "w") as f:
    f.write(code)
