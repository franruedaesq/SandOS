//! # Web Server Task — SandOS Dashboard
//!
//! Lightweight HTTP/1.0 server running on port 80.
//!
//! Routes:
//! - `GET /`           → full HTML dashboard (glassmorphism style)
//! - `GET /api/stats`  → JSON snapshot of system metrics
//!
//! Access via: `http://<ESP32-IP>/`

use embassy_executor::task;
use embassy_net::{tcp::TcpSocket, Stack};
use embassy_time::{Duration, Instant, Timer};
use embedded_io_async::Write;

use portable_atomic::AtomicBool;
use core::sync::atomic::Ordering;
use crate::led_state;

// ── Web server enable/disable toggle ─────────────────────────────────────────

/// Starts **disabled** — the display menu toggles it on.
/// The web UI `/api/server/stop` endpoint turns it off.
pub static WEB_SERVER_ENABLED: AtomicBool = AtomicBool::new(false);

/// Enable the web server (called from display menu).
pub fn enable_web_server() {
    WEB_SERVER_ENABLED.store(true, Ordering::Relaxed);
    log::info!("[web_server] ENABLED by user");
}

/// Disable the web server (called from web UI or display menu).
pub fn disable_web_server() {
    WEB_SERVER_ENABLED.store(false, Ordering::Relaxed);
    log::info!("[web_server] DISABLED by user");
}

pub fn is_web_server_enabled() -> bool {
    WEB_SERVER_ENABLED.load(Ordering::Relaxed)
}

// ── Boot time ────────────────────────────────────────────────────────────────

/// Ticks recorded when the web server first becomes active.
/// 0 means "not yet set" (server has not been enabled yet).
static BOOT_SERVER_STARTED_TICKS: portable_atomic::AtomicU64 =
    portable_atomic::AtomicU64::new(0);

// ── HTML Dashboard ────────────────────────────────────────────────────────────

const HTML_DASHBOARD: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>SandOS Kawaii Face UI</title>
    <style>
        @import url('https://fonts.googleapis.com/css2?family=Nunito:wght@500;700;800&display=swap');
        * { margin: 0; padding: 0; box-sizing: border-box; }
        body {
            font-family: 'Nunito', 'Segoe UI', sans-serif;
            background: radial-gradient(circle at top, #ffd5ec 0%, #e8d6ff 45%, #cbe9ff 100%);
            min-height: 100vh;
            display: grid;
            place-items: center;
            color: #5a456b;
            overflow: hidden;
        }
        .scene {
            width: min(92vw, 580px);
            text-align: center;
        }
        h1 {
            font-size: clamp(1.6rem, 4vw, 2.2rem);
            margin-bottom: 10px;
            color: #7d4f8e;
        }
        .subtitle {
            font-size: 0.98rem;
            opacity: 0.8;
            margin-bottom: 24px;
        }

        .face-shell {
            position: relative;
            width: min(78vw, 360px);
            aspect-ratio: 1 / 1;
            margin: 0 auto;
        }
        .face {
            width: 100%;
            height: 100%;
            border-radius: 46% 46% 42% 42%;
            background: linear-gradient(160deg, #fff4fb 0%, #ffe8f6 48%, #ffe3ec 100%);
            border: 4px solid rgba(255, 255, 255, 0.9);
            box-shadow: 0 18px 45px rgba(127, 89, 141, 0.25), inset 0 6px 18px rgba(255, 255, 255, 0.9);
            position: relative;
            cursor: pointer;
            user-select: none;
            animation: bob 3.5s ease-in-out infinite;
        }
        .eyes {
            position: absolute;
            left: 0;
            right: 0;
            top: 35%;
            display: flex;
            justify-content: center;
            gap: 56px;
        }
        .eye {
            position: relative;
            width: 62px;
            height: 78px;
            border-radius: 40px;
            background: #3e3154;
            overflow: hidden;
            transform-origin: center 70%;
            transition: transform 220ms ease;
        }
        .eye::before,
        .eye::after {
            content: '';
            position: absolute;
            border-radius: 50%;
            background: rgba(255, 255, 255, 0.95);
        }
        .eye::before {
            width: 20px;
            height: 20px;
            top: 14px;
            left: 14px;
        }
        .eye::after {
            width: 10px;
            height: 10px;
            top: 38px;
            left: 28px;
        }

        .mouth {
            position: absolute;
            left: 50%;
            top: 66%;
            width: 58px;
            height: 26px;
            border: 5px solid #b44f79;
            border-top: 0;
            border-radius: 0 0 50px 50px;
            transform: translateX(-50%);
            transition: all 250ms ease;
        }
        .cheek {
            position: absolute;
            top: 59%;
            width: 52px;
            height: 24px;
            border-radius: 999px;
            background: rgba(255, 132, 175, 0.35);
            filter: blur(1px);
            transition: opacity 220ms ease;
        }
        .cheek.left { left: 17%; }
        .cheek.right { right: 17%; }

        .menu {
            margin-top: 24px;
            display: grid;
            grid-template-columns: repeat(3, minmax(0, 1fr));
            gap: 12px;
            opacity: 0;
            transform: translateY(8px) scale(0.98);
            pointer-events: none;
            transition: all 230ms ease;
        }
        .menu.show {
            opacity: 1;
            transform: translateY(0) scale(1);
            pointer-events: auto;
        }
        .menu button {
            border: 0;
            border-radius: 14px;
            background: rgba(255, 255, 255, 0.72);
            color: #7a4d87;
            font-weight: 800;
            padding: 12px 10px;
            box-shadow: 0 8px 20px rgba(125, 84, 143, 0.14);
            cursor: pointer;
            transition: all 180ms ease;
        }
        .menu button.active,
        .menu button:hover {
            background: #fff;
            color: #d9559a;
            box-shadow: 0 0 0 2px rgba(255, 166, 214, 0.55), 0 10px 24px rgba(220, 91, 154, 0.25);
        }

        .status {
            margin-top: 14px;
            font-size: 0.9rem;
            color: #775886;
            opacity: 0.95;
        }
        .sparkle {
            position: absolute;
            width: 10px;
            height: 10px;
            border-radius: 50%;
            background: #fff;
            box-shadow: 0 0 18px rgba(255, 255, 255, 0.95);
            opacity: 0;
        }
        .sparkle.left { top: 25%; left: 14%; }
        .sparkle.right { top: 24%; right: 14%; }

        .face.blink .eye { transform: scaleY(0.08); }
        .face.wink-left .eye.left { transform: scaleY(0.08); }
        .face.wink-right .eye.right { transform: scaleY(0.08); }

        .face.smile .mouth {
            width: 70px;
            height: 30px;
            border-color: #b83f76;
        }
        .face.smile .sparkle {
            opacity: 1;
            animation: twinkle 420ms ease;
        }
        .face.surprised .eye {
            transform: scale(1.14);
        }
        .face.surprised .mouth {
            width: 30px;
            height: 30px;
            border-radius: 50%;
            border: 5px solid #b44f79;
            background: rgba(255, 205, 225, 0.65);
        }
        .face.surprised .cheek {
            background: rgba(255, 120, 160, 0.48);
        }

        @keyframes bob {
            0%, 100% { transform: translateY(0px); }
            50% { transform: translateY(-6px); }
        }
        @keyframes twinkle {
            0% { transform: scale(0.2); opacity: 0; }
            40% { transform: scale(1.2); opacity: 1; }
            100% { transform: scale(1); opacity: 0; }
        }

        @media (max-width: 480px) {
            .menu { grid-template-columns: repeat(2, minmax(0, 1fr)); }
        }
    </style>
</head>
<body>
<main class="scene">
    <h1>SandOS Kawaii Face</h1>
    <p class="subtitle">Tap the face to open a cute menu ✨</p>

    <div class="face-shell">
        <div class="face" id="face" role="button" tabindex="0" aria-label="Open menu">
            <div class="sparkle left"></div>
            <div class="sparkle right"></div>
            <div class="eyes">
                <div class="eye left"></div>
                <div class="eye right"></div>
            </div>
            <div class="mouth" id="mouth"></div>
            <div class="cheek left"></div>
            <div class="cheek right"></div>
        </div>
    </div>

    <section class="menu" id="menu" aria-label="Face options menu">
        <button data-item="Home">🏠 Home</button>
        <button data-item="Settings">⚙️ Settings</button>
        <button data-item="Profile">👤 Profile</button>
        <button data-item="Gallery">🖼️ Gallery</button>
        <button data-item="About">⭐ About</button>
    </section>

    <p class="status" id="status">Waiting for a tap...</p>
</main>

<script>
const face = document.getElementById('face');
const menu = document.getElementById('menu');
const status = document.getElementById('status');
const menuButtons = Array.from(menu.querySelectorAll('button'));

let menuOpen = false;
let currentExpression = null;

function setExpression(name, duration = 520) {
    if (currentExpression) face.classList.remove(currentExpression);
    currentExpression = name;
    if (name) {
        face.classList.add(name);
        setTimeout(() => {
            face.classList.remove(name);
            if (currentExpression === name) currentExpression = null;
        }, duration);
    }
}

function toggleMenu() {
    menuOpen = !menuOpen;
    menu.classList.toggle('show', menuOpen);
    status.textContent = menuOpen
        ? 'Menu opened! Pick an option 💖'
        : 'Menu hidden. Tap again!';
    setExpression(menuOpen ? 'smile' : 'blink', menuOpen ? 620 : 280);
}

face.addEventListener('click', toggleMenu);
face.addEventListener('touchstart', (e) => { e.preventDefault(); toggleMenu(); }, { passive: false });
face.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' || e.key === ' ') {
        e.preventDefault();
        toggleMenu();
    }
});

menuButtons.forEach((button) => {
    button.addEventListener('click', () => {
        menuButtons.forEach((b) => b.classList.remove('active'));
        button.classList.add('active');
        status.textContent = `Selected: ${button.dataset.item}`;
        setExpression('wink-left', 360);
    });
});

function idleLoop() {
    const moods = [
        ['blink', 220],
        ['smile', 600],
        ['surprised', 650],
        ['wink-right', 350],
        ['blink', 260],
    ];
    const [name, duration] = moods[Math.floor(Math.random() * moods.length)];
    setExpression(name, duration);
    const next = 2200 + Math.random() * 2200;
    setTimeout(idleLoop, next);
}

setTimeout(idleLoop, 1200);
</script>
</body>
</html>"#;

// ── Task ──────────────────────────────────────────────────────────────────────

/// HTTP server task — listens on port 80, serves the SandOS dashboard.
#[task]
pub async fn web_server_task(stack: &'static Stack<'static>) {
    // Don't touch the network until the user enables us from the menu.
    // This avoids wait_config_up() starving the display during DHCP.
    log::info!("[web_server] idle (disabled by default — enable from BOOT menu)");

    let mut rx_buf = [0u8; 1024];
    let mut tx_buf = [0u8; 8192];

    let mut was_enabled = false;

    loop {
        // Sleep while disabled — display menu or web UI can toggle.
        while !is_web_server_enabled() {
            was_enabled = false;
            Timer::after(Duration::from_millis(500)).await;
        }

        // First time enabled (or re-enabled): wait for network + log once.
        if !was_enabled {
            if stack.config_v4().is_none() {
                log::info!("[web_server] waiting for network…");
                let wait_started = Instant::now();
                let mut last_wait_log = wait_started;

                while stack.config_v4().is_none() {
                    if !is_web_server_enabled() {
                        log::info!("[web_server] waiting cancelled (server disabled)");
                        break;
                    }

                    let now = Instant::now();
                    if now - last_wait_log >= Duration::from_secs(3) {
                        let elapsed = (now - wait_started).as_secs();
                        let wifi_status = crate::wifi::wifi_status();
                        if let Some(ip) = crate::wifi::wifi_ipv4() {
                            log::info!(
                                "[web_server] waiting {}s (wifi_status={}, ip={}.{}.{}.{})",
                                elapsed,
                                wifi_status,
                                ip[0], ip[1], ip[2], ip[3]
                            );
                        } else {
                            log::info!(
                                "[web_server] waiting {}s (wifi_status={}, ip=none)",
                                elapsed,
                                wifi_status
                            );
                        }
                        if elapsed >= 30 {
                            log::warn!(
                                "[web_server] network still unavailable after {}s",
                                elapsed
                            );
                        }
                        last_wait_log = now;
                    }

                    Timer::after(Duration::from_millis(250)).await;
                }
            }

            if !is_web_server_enabled() {
                continue;
            }
            log::info!("[web_server] listening on port 80");
            // Record the tick when the server first becomes active (write-once).
            BOOT_SERVER_STARTED_TICKS.compare_exchange(
                0,
                embassy_time::Instant::now().as_ticks(),
                core::sync::atomic::Ordering::Relaxed,
                core::sync::atomic::Ordering::Relaxed,
            ).ok();
            was_enabled = true;
        }

        let mut socket = TcpSocket::new(*stack, &mut rx_buf, &mut tx_buf);
        socket.set_timeout(Some(Duration::from_secs(10)));

        if let Err(e) = socket.accept(80).await {
            log::warn!("[web_server] accept error: {:?}", e);
            continue;
        }

        // Read enough of the request to identify the path.
        let mut req_buf = [0u8; 512];
        let n = match socket.read(&mut req_buf).await {
            Ok(n) if n > 0 => n,
            _ => { socket.close(); continue; }
        };

        let req = core::str::from_utf8(&req_buf[..n]).unwrap_or("");
        // Only log non-routine requests to avoid UART flooding.
        if !req.starts_with("GET /api/stats") {
            log::info!("[web_server] {} bytes", n);
        }

        if req.starts_with("POST /api/server/stop") {
            // Send response BEFORE disabling so the browser gets a reply.
            let body = r#"{"status":"stopping"}"#;
            let mut hdr: heapless::String<128> = heapless::String::new();
            let _ = core::fmt::write(&mut hdr, format_args!(
                "HTTP/1.0 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            ));
            let _ = socket.write_all(hdr.as_bytes()).await;
            let _ = socket.write_all(body.as_bytes()).await;
            let _ = socket.flush().await;
            socket.close();
            disable_web_server();
            continue;
        } else if req.starts_with("GET /api/stats") {
            serve_api_stats(&mut socket).await;
        } else if req.starts_with("GET /api/led/get") {
            serve_led_get(&mut socket).await;
        } else if req.starts_with("POST /api/led/set") {
            let body = extract_http_body(&req_buf[..n]);
            serve_led_set(&mut socket, body).await;
        } else {
            serve_dashboard(&mut socket).await;
        }

        socket.close();

        // Rate-limit: yield 20 ms between requests so the display task
        // (and other Core 0 tasks) get regular executor time.  Without
        // this, back-to-back HTTP requests starve the display loop.
        Timer::after(Duration::from_millis(20)).await;
    }
}

// ── Helper: Extract HTTP body ─────────────────────────────────────────────────

/// Extract the HTTP body from a complete HTTP request
fn extract_http_body(buf: &[u8]) -> &str {
    // Find the blank line that separates headers from body ("\r\n\r\n")
    for i in 0..buf.len().saturating_sub(3) {
        if buf[i] == b'\r' && buf[i+1] == b'\n' && buf[i+2] == b'\r' && buf[i+3] == b'\n' {
            // Found the separator; body starts at i+4
            let body_start = i + 4;
            if body_start < buf.len() {
                return core::str::from_utf8(&buf[body_start..]).unwrap_or("");
            }
            return "";
        }
    }
    ""
}

// ── Handlers ─────────────────────────────────────────────────────────────────

async fn serve_dashboard(socket: &mut TcpSocket<'_>) {
    let body = HTML_DASHBOARD.as_bytes();

    let mut hdr: heapless::String<128> = heapless::String::new();
    let _ = core::fmt::write(
        &mut hdr,
        format_args!(
            "HTTP/1.0 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        ),
    );

    let _ = socket.write_all(hdr.as_bytes()).await;
    let _ = socket.write_all(body).await;
    let _ = socket.flush().await;
}

async fn serve_api_stats(socket: &mut TcpSocket<'_>) {
    // Uptime since the web server was first enabled.
    let start_ticks = BOOT_SERVER_STARTED_TICKS.load(core::sync::atomic::Ordering::Relaxed);
    let uptime_secs = if start_ticks > 0 {
        embassy_time::Instant::now()
            .duration_since(embassy_time::Instant::from_ticks(start_ticks))
            .as_secs()
    } else {
        0
    };

    // Heap stats via esp-alloc
    let psram_free = esp_alloc::HEAP.free();
    let psram_used = esp_alloc::HEAP.used();

    // Hot-swap count
    let hot_swaps = crate::core0::HOT_SWAP_COUNT.load(core::sync::atomic::Ordering::Relaxed);

    let mut json: heapless::String<256> = heapless::String::new();
    let _ = core::fmt::write(
        &mut json,
        format_args!(
            r#"{{"uptime_secs":{},"psram_free":{},"psram_used":{},"hot_swaps":{},"ip":"connected"}}"#,
            uptime_secs, psram_free, psram_used, hot_swaps
        ),
    );

    let mut hdr: heapless::String<128> = heapless::String::new();
    let _ = core::fmt::write(
        &mut hdr,
        format_args!(
            "HTTP/1.0 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            json.len()
        ),
    );

    let _ = socket.write_all(hdr.as_bytes()).await;
    let _ = socket.write_all(json.as_bytes()).await;
    let _ = socket.flush().await;
}

// ── RGB LED State ─────────────────────────────────────────────────────────────

async fn serve_led_get(socket: &mut TcpSocket<'_>) {
    let (r, g, b) = led_state::get_led_color();
    let mut json: heapless::String<64> = heapless::String::new();
    let _ = core::fmt::write(
        &mut json,
        format_args!(r#"{{"r":{},"g":{},"b":{}}}"#, r, g, b),
    );

    let mut hdr: heapless::String<128> = heapless::String::new();
    let _ = core::fmt::write(
        &mut hdr,
        format_args!(
            "HTTP/1.0 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            json.len()
        ),
    );

    let _ = socket.write_all(hdr.as_bytes()).await;
    let _ = socket.write_all(json.as_bytes()).await;
    let _ = socket.flush().await;
}

async fn serve_led_set(socket: &mut TcpSocket<'_>, body: &str) {
    log::info!("[web_server] LED set body: '{}'", body);

    // Parse r=R&g=G&b=B from form data
    let mut r: u8 = 0;
    let mut g: u8 = 0;
    let mut b: u8 = 0;
    let mut valid = true;

    // Simple parameter parsing
    for param in body.split('&') {
        if let Some((key, val)) = param.split_once('=') {
            match val.parse::<u8>() {
                Ok(num) => {
                    match key {
                        "r" => r = num,
                        "g" => g = num,
                        "b" => b = num,
                        _ => {}
                    }
                }
                Err(_) => {
                    log::warn!("[web_server] Failed to parse {} = {}", key, val);
                    valid = false;
                }
            }
        }
    }

    log::info!("[web_server] Parsed LED: R={} G={} B={}, valid={}", r, g, b, valid);

    // Update the LED state via the led_state module
    if valid {
        led_state::set_led_color(r, g, b);
        log::info!("[web_server] LED color updated successfully");
    }

    // Send JSON response
    let mut json: heapless::String<80> = heapless::String::new();
    let status_str = if valid { "ok" } else { "error" };
    let _ = core::fmt::write(
        &mut json,
        format_args!(r#"{{"status":"{}","r":{},"g":{},"b":{}}}"#, status_str, r, g, b),
    );

    let http_status = if valid { "200 OK" } else { "400 Bad Request" };
    let mut hdr: heapless::String<256> = heapless::String::new();
    let _ = core::fmt::write(
        &mut hdr,
        format_args!(
            "HTTP/1.0 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            http_status, json.len()
        ),
    );

    let _ = socket.write_all(hdr.as_bytes()).await;
    let _ = socket.write_all(json.as_bytes()).await;
    let _ = socket.flush().await;
}
