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
use embassy_time::{Duration, Instant};
use embedded_io_async::Write;

use crate::led_state;

// ── Boot time ────────────────────────────────────────────────────────────────

static mut BOOT_TIME: Option<Instant> = None;

// ── HTML Dashboard ────────────────────────────────────────────────────────────

const HTML_DASHBOARD: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>SandOS Dashboard</title>
    <style>
        @import url('https://fonts.googleapis.com/css2?family=Inter:wght@300;400;600;700&display=swap');
        * { margin: 0; padding: 0; box-sizing: border-box; }
        body {
            font-family: 'Inter', 'Segoe UI', sans-serif;
            background: linear-gradient(135deg, #0f0c29 0%, #302b63 50%, #24243e 100%);
            color: #fff;
            min-height: 100vh;
            padding: 24px;
        }
        .container { max-width: 860px; margin: 0 auto; }
        header { text-align: center; margin-bottom: 32px; }
        header h1 {
            font-size: 2.8em;
            font-weight: 700;
            background: linear-gradient(90deg, #a78bfa, #60a5fa);
            -webkit-background-clip: text;
            -webkit-text-fill-color: transparent;
            margin-bottom: 6px;
        }
        header p { opacity: 0.6; font-size: 1em; letter-spacing: 1px; }
        .chip-badge {
            display: inline-block;
            background: rgba(167,139,250,0.15);
            border: 1px solid rgba(167,139,250,0.3);
            border-radius: 999px;
            padding: 6px 18px;
            font-size: 0.85em;
            margin-top: 12px;
            letter-spacing: 0.5px;
        }
        .grid {
            display: grid;
            grid-template-columns: repeat(auto-fit, minmax(240px, 1fr));
            gap: 18px;
            margin-bottom: 24px;
        }
        .card {
            background: rgba(255,255,255,0.07);
            backdrop-filter: blur(16px);
            border: 1px solid rgba(255,255,255,0.12);
            border-radius: 18px;
            padding: 24px;
            box-shadow: 0 8px 32px rgba(0,0,0,0.3);
            transition: transform 0.25s ease, box-shadow 0.25s ease;
        }
        .card:hover { transform: translateY(-4px); box-shadow: 0 14px 40px rgba(0,0,0,0.45); }
        .card-label {
            font-size: 0.75em;
            font-weight: 600;
            letter-spacing: 1.5px;
            text-transform: uppercase;
            opacity: 0.55;
            margin-bottom: 12px;
        }
        .card-value {
            font-size: 2.4em;
            font-weight: 700;
            line-height: 1;
            margin-bottom: 4px;
        }
        .card-sub { font-size: 0.85em; opacity: 0.55; }
        .online  { color: #4ade80; }
        .offline { color: #f87171; }
        .bar {
            height: 8px;
            background: rgba(255,255,255,0.1);
            border-radius: 4px;
            overflow: hidden;
            margin-top: 12px;
        }
        .bar-fill {
            height: 100%;
            background: linear-gradient(90deg, #a78bfa, #60a5fa);
            border-radius: 4px;
            transition: width 0.5s ease;
        }
        footer {
            text-align: center;
            opacity: 0.35;
            font-size: 0.8em;
            margin-top: 8px;
        }
        .pulse { animation: pulse 2s infinite; }
        @keyframes pulse {
            0%,100% { opacity: 1; }
            50%      { opacity: 0.4; }
        }
    </style>
</head>
<body>
<div class="container">
    <header>
        <h1>🪣 SandOS</h1>
        <p>LIVE SYSTEM DASHBOARD</p>
        <span class="chip-badge">ESP32-S3 · Xtensa LX7 Dual-Core · 240 MHz</span>
    </header>

    <div class="grid">
        <div class="card">
            <div class="card-label">WiFi Status</div>
            <div class="card-value online" id="wifi-status">●</div>
            <div class="card-sub" id="wifi-ip">--</div>
        </div>

        <div class="card">
            <div class="card-label">System Uptime</div>
            <div class="card-value" id="uptime">--</div>
            <div class="card-sub">seconds</div>
        </div>

        <div class="card">
            <div class="card-label">Free PSRAM</div>
            <div class="card-value" id="psram-free">--</div>
            <div class="card-sub" id="psram-sub">KB available</div>
            <div class="bar"><div class="bar-fill" id="psram-bar" style="width:0%"></div></div>
        </div>

        <div class="card">
            <div class="card-label">Wasm Hot-Swaps</div>
            <div class="card-value" id="hotswaps">--</div>
            <div class="card-sub">OTA swaps completed</div>
        </div>

        <div class="card">
            <div class="card-label">Wasm Engine</div>
            <div class="card-value online pulse" id="wasm-status">●</div>
            <div class="card-sub">Running</div>
        </div>

        <div class="card">
            <div class="card-label">Heartbeat</div>
            <div class="card-value online pulse" id="heartbeat">●</div>
            <div class="card-sub">Core 0 alive</div>
        </div>
    </div>

    <div class="card" style="grid-column: 1 / -1;">
        <div class="card-label">RGB LED Control</div>
        <div id="led-display" style="width: 100%; height: 60px; border-radius: 12px; background: rgb(0, 0, 0); margin: 12px 0; box-shadow: inset 0 2px 8px rgba(0,0,0,0.5);"></div>
        <div style="display: grid; grid-template-columns: repeat(3, 1fr); gap: 8px; margin-bottom: 12px;">
            <button onclick="setLED(255,0,0)" style="padding:10px; background:rgba(255,0,0,0.3); border:1px solid #f87171; color:#f87171; border-radius:8px; cursor:pointer; font-weight:600;">Red</button>
            <button onclick="setLED(0,255,0)" style="padding:10px; background:rgba(0,255,0,0.3); border:1px solid #4ade80; color:#4ade80; border-radius:8px; cursor:pointer; font-weight:600;">Green</button>
            <button onclick="setLED(0,0,255)" style="padding:10px; background:rgba(0,0,255,0.3); border:1px solid #60a5fa; color:#60a5fa; border-radius:8px; cursor:pointer; font-weight:600;">Blue</button>
            <button onclick="setLED(255,255,0)" style="padding:10px; background:rgba(255,255,0,0.3); border:1px solid #fbbf24; color:#fbbf24; border-radius:8px; cursor:pointer; font-weight:600;">Yellow</button>
            <button onclick="setLED(255,0,255)" style="padding:10px; background:rgba(255,0,255,0.3); border:1px solid #d946ef; color:#d946ef; border-radius:8px; cursor:pointer; font-weight:600;">Magenta</button>
            <button onclick="setLED(255,255,255)" style="padding:10px; background:rgba(255,255,255,0.3); border:1px solid #e5e7eb; color:#e5e7eb; border-radius:8px; cursor:pointer; font-weight:600;">White</button>
            <button onclick="setLED(0,255,255)" style="padding:10px; background:rgba(0,255,255,0.3); border:1px solid #22d3ee; color:#22d3ee; border-radius:8px; cursor:pointer; font-weight:600;">Cyan</button>
            <button onclick="setLED(0,0,0)" style="padding:10px; background:rgba(100,100,100,0.3); border:1px solid #999; color:#999; border-radius:8px; cursor:pointer; font-weight:600;">Off</button>
        </div>
        <div style="font-size:0.85em; opacity:0.6;">RGB: <span id="led-values">0, 0, 0</span></div>
    </div>

    <footer>SandOS v0.1.0 · Embassy + Rust 🦀 · Refreshing every 2 s</footer>
</div>

<script>
async function setLED(r, g, b) {
    try {
        const res = await fetch('/api/led/set', {
            method: 'POST',
            headers: {'Content-Type': 'application/x-www-form-urlencoded'},
            body: `r=${r}&g=${g}&b=${b}`
        });
        if (res.ok) {
            document.getElementById('led-display').style.background = `rgb(${r},${g},${b})`;
            document.getElementById('led-values').textContent = `${r}, ${g}, ${b}`;
        }
    } catch (e) {
        console.log('LED set failed:', e);
    }
}

async function refreshLED() {
    try {
        const res = await fetch('/api/led/get');
        const data = await res.json();
        document.getElementById('led-display').style.background = `rgb(${data.r},${data.g},${data.b})`;
        document.getElementById('led-values').textContent = `${data.r}, ${data.g}, ${data.b}`;
    } catch (e) {
        // LED endpoint may not be available yet
    }
}

async function refresh() {
    try {
        const r = await fetch('/api/stats');
        const d = await r.json();

        document.getElementById('wifi-status').textContent = '● ONLINE';
        document.getElementById('wifi-ip').textContent = d.ip || '(DHCP)';
        document.getElementById('uptime').textContent = d.uptime_secs;

        const freeKB = Math.round(d.psram_free / 1024);
        const usedKB = Math.round(d.psram_used / 1024);
        const pct = d.psram_used + d.psram_free > 0
            ? Math.round(d.psram_used / (d.psram_used + d.psram_free) * 100) : 0;
        document.getElementById('psram-free').textContent = freeKB;
        document.getElementById('psram-sub').textContent = `KB free of ${freeKB + usedKB} KB`;
        document.getElementById('psram-bar').style.width = pct + '%';

        document.getElementById('hotswaps').textContent = d.hot_swaps;

        // Refresh LED status
        setTimeout(refreshLED, 500);
    } catch (e) {
        document.getElementById('wifi-status').className = 'card-value offline';
        document.getElementById('wifi-status').textContent = '○ OFFLINE';
    }
}
refresh();
setInterval(refresh, 2000);
</script>
</body>
</html>"#;

// ── Task ──────────────────────────────────────────────────────────────────────

/// HTTP server task — listens on port 80, serves the SandOS dashboard.
#[task]
pub async fn web_server_task(stack: &'static Stack<'static>) {
    log::info!("[web_server] waiting for network…");
    stack.wait_config_up().await;
    log::info!("[web_server] listening on port 80");

    unsafe { BOOT_TIME = Some(Instant::now()); }

    let mut rx_buf = [0u8; 1024];
    let mut tx_buf = [0u8; 8192];

    loop {
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
        log::info!("[web_server] {} bytes", n);

        if req.starts_with("GET /api/stats") {
            serve_api_stats(&mut socket).await;
        } else if req.starts_with("GET /api/led/get") {
            serve_led_get(&mut socket).await;
        } else if req.starts_with("POST /api/led/set") {
            // Extract the POST body from the request buffer
            let body = extract_http_body(&req_buf[..n]);
            serve_led_set(&mut socket, body).await;
        } else {
            serve_dashboard(&mut socket).await;
        }

        socket.close();
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
    // Uptime
    let uptime_secs = unsafe {
        BOOT_TIME
            .map(|t| Instant::now().duration_since(t).as_secs())
            .unwrap_or(0)
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
