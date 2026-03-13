#![allow(dead_code)]
//! Fetches real-time departure data from an HTTP endpoint
//! and stores it in shared state for the display task to render.
//!
//! The endpoint must serve plain HTTP (no TLS) because the ESP32-S3
//! doesn't have enough internal SRAM for an embedded TLS stack alongside
//! the WiFi driver.
//!
//! ## JSON format (v2 — nested by station → direction)
//!
//! ```json
//! {
//!   "Floridsdorf": {
//!     "SIEBENHIRTEN": [{"l":"U6","t":[{"e":2,"h":"09:00"},{"e":6,"h":"09:04"}]}]
//!   },
//!   "Martinstraße": {
//!     "Schottentor U": [{"l":"40","t":[...]},{"l":"41","t":[...]}]
//!   }
//! }
//! ```

use core::cell::RefCell;
use embassy_executor::task;
use embassy_net::tcp::TcpSocket;
use embassy_net::Stack;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex;
use embassy_time::{Duration, Instant, Timer};
use embedded_io_async::Write;

/// HTTP endpoint — must be plain HTTP (port 80), no HTTPS.
const HOST_IP: [u8; 4] = [192, 168, 0, 164];
const HOST_PORT: u16 = 3000;
const HOST_HEADER: &str = "192.168.0.164:3000";
const FETCH_INTERVAL_SECS: u64 = 60;

// ── Shared types ────────────────────────────────────────────────────────────

#[derive(Clone, Default)]
pub struct Departure {
    pub wait_minutes: u16,
    pub time_str: heapless::String<8>,
}

#[derive(Clone, Default)]
pub struct RouteEntry {
    pub line: heapless::String<8>,
    pub departures: heapless::Vec<Departure, 4>,
}

/// One station+direction combination with all its routes.
#[derive(Clone, Default)]
pub struct StopDirection {
    pub station: heapless::String<32>,
    pub direction: heapless::String<32>,
    pub routes: heapless::Vec<RouteEntry, 4>,
}

#[derive(Clone)]
pub struct FetchedLines {
    pub stops: heapless::Vec<StopDirection, 16>,
    pub last_update_ms: u64,
    pub error: bool,
    pub loading: bool,
}

impl Default for FetchedLines {
    fn default() -> Self {
        Self {
            stops: heapless::Vec::new(),
            last_update_ms: 0,
            error: false,
            loading: false,
        }
    }
}

// ── Shared state ────────────────────────────────────────────────────────────

static SHARED: Mutex<CriticalSectionRawMutex, RefCell<FetchedLines>> =
    Mutex::new(RefCell::new(FetchedLines {
        stops: heapless::Vec::new(),
        last_update_ms: 0,
        error: false,
        loading: false,
    }));

pub fn get_lines() -> FetchedLines {
    SHARED.lock(|cell| cell.borrow().clone())
}

fn store(data: FetchedLines) {
    SHARED.lock(|cell| {
        *cell.borrow_mut() = data;
    });
}

// ── Fetch task ──────────────────────────────────────────────────────────────

#[task]
pub async fn vienna_fetch_task(stack: &'static Stack<'static>) {
    log::info!("[vienna] fetch task started — waiting for network");

    loop {
        if stack.config_v4().is_some() {
            break;
        }
        Timer::after(Duration::from_secs(1)).await;
    }
    log::info!("[vienna] network ready — starting fetch loop");

    loop {
        {
            let mut current = get_lines();
            current.loading = true;
            store(current);
        }

        match fetch_once(stack).await {
            Ok(data) => {
                log::info!("[vienna] fetched {} stops", data.stops.len());
                store(data);
            }
            Err(e) => {
                log::warn!("[vienna] fetch error: {}", e);
                let mut d = FetchedLines::default();
                d.error = true;
                store(d);
            }
        }

        Timer::after(Duration::from_secs(FETCH_INTERVAL_SECS)).await;
    }
}

async fn fetch_once(stack: &'static Stack<'static>) -> Result<FetchedLines, &'static str> {
    let remote = embassy_net::IpAddress::v4(HOST_IP[0], HOST_IP[1], HOST_IP[2], HOST_IP[3]);

    // ── TCP ──
    let mut rx_buf = [0u8; 2048];
    let mut tx_buf = [0u8; 256];
    let mut socket = TcpSocket::new(*stack, &mut rx_buf, &mut tx_buf);
    socket.set_timeout(Some(Duration::from_secs(10)));

    socket
        .connect((remote, HOST_PORT))
        .await
        .map_err(|_| "TCP connect failed")?;

    // ── HTTP request ──
    let mut req_buf: heapless::Vec<u8, 256> = heapless::Vec::new();
    let _ = req_buf.extend_from_slice(b"GET / HTTP/1.1\r\nHost: ");
    let _ = req_buf.extend_from_slice(HOST_HEADER.as_bytes());
    let _ = req_buf.extend_from_slice(b"\r\nConnection: close\r\nAccept: application/json\r\n\r\n");

    socket
        .write_all(&req_buf)
        .await
        .map_err(|_| "HTTP write failed")?;

    // ── Read response ──
    let mut buf = [0u8; 4096];
    let mut total = 0usize;
    loop {
        match socket.read(&mut buf[total..]).await {
            Ok(0) => break,
            Ok(n) => {
                total += n;
                if total >= buf.len() {
                    break;
                }
            }
            Err(_) => break,
        }
    }

    if total == 0 {
        return Err("empty response");
    }

    let resp = core::str::from_utf8(&buf[..total]).map_err(|_| "non-UTF8 response")?;

    // Check HTTP status
    let status_ok = resp.starts_with("HTTP/1.1 200") || resp.starts_with("HTTP/1.0 200");
    if !status_ok {
        let end = resp.find('\r').unwrap_or(60).min(60);
        log::warn!("[vienna] HTTP status: {}", &resp[..end]);
        return Err("non-200 HTTP status");
    }

    // Find body after \r\n\r\n
    let body_start = resp.find("\r\n\r\n").ok_or("no HTTP body separator")? + 4;
    let body = &resp[body_start..];

    log::info!("[vienna] response body: {} bytes", body.len());
    parse_json(body)
}

// ── JSON parser (v2 — nested station → direction → routes) ──────────────────
//
// Parses:
//   { "Station": { "Direction": [{"l":"U6","t":[{"e":2,"h":"09:00"},...]}, ...], ... }, ... }

fn skip_ws(b: &[u8], mut pos: usize) -> usize {
    while pos < b.len() && matches!(b[pos], b' ' | b'\t' | b'\n' | b'\r') {
        pos += 1;
    }
    pos
}

fn expect(b: &[u8], pos: usize, ch: u8) -> Result<usize, &'static str> {
    let p = skip_ws(b, pos);
    if p < b.len() && b[p] == ch {
        Ok(p + 1)
    } else {
        Err("unexpected char")
    }
}

/// Parse a JSON string starting at `pos`. Returns (string_slice, position_after_closing_quote).
fn parse_str<'a>(b: &'a [u8], pos: usize) -> Result<(&'a str, usize), &'static str> {
    let p = skip_ws(b, pos);
    if p >= b.len() || b[p] != b'"' {
        return Err("expected string");
    }
    let start = p + 1;
    let mut end = start;
    while end < b.len() && b[end] != b'"' {
        if b[end] == b'\\' {
            end += 1;
        }
        end += 1;
    }
    if end >= b.len() {
        return Err("unterminated string");
    }
    let s = core::str::from_utf8(&b[start..end]).map_err(|_| "non-UTF8")?;
    Ok((s, end + 1))
}

fn parse_num(b: &[u8], pos: usize) -> (u16, usize) {
    let p = skip_ws(b, pos);
    let mut val: u16 = 0;
    let mut i = p;
    while i < b.len() && b[i].is_ascii_digit() {
        val = val.saturating_mul(10).saturating_add((b[i] - b'0') as u16);
        i += 1;
    }
    (val, i)
}

fn parse_json(body: &str) -> Result<FetchedLines, &'static str> {
    let b = body.as_bytes();
    let mut result = FetchedLines {
        stops: heapless::Vec::new(),
        last_update_ms: Instant::now().as_millis(),
        error: false,
        loading: false,
    };

    let mut pos = expect(b, 0, b'{')?;

    // Iterate over station entries
    loop {
        pos = skip_ws(b, pos);
        if pos >= b.len() || b[pos] == b'}' {
            break;
        }
        if b[pos] == b',' {
            pos += 1;
            continue;
        }

        // Station name
        let (station, next) = parse_str(b, pos)?;
        pos = expect(b, next, b':')?;
        pos = expect(b, pos, b'{')?;

        // Iterate over direction entries within this station
        loop {
            pos = skip_ws(b, pos);
            if pos >= b.len() || b[pos] == b'}' {
                pos += 1;
                break;
            }
            if b[pos] == b',' {
                pos += 1;
                continue;
            }

            // Direction name
            let (direction, next) = parse_str(b, pos)?;
            pos = expect(b, next, b':')?;
            pos = expect(b, pos, b'[')?;

            let mut stop = StopDirection::default();
            let _ = stop.station.push_str(station);
            let _ = stop.direction.push_str(direction);

            // Array of route objects [{"l":"U6","t":[...]}, ...]
            loop {
                pos = skip_ws(b, pos);
                if pos >= b.len() || b[pos] == b']' {
                    pos += 1;
                    break;
                }
                if b[pos] == b',' {
                    pos += 1;
                    continue;
                }

                // Route object: {"l":"...","t":[...]}
                pos = expect(b, pos, b'{')?;
                let mut route = RouteEntry::default();

                loop {
                    pos = skip_ws(b, pos);
                    if pos >= b.len() || b[pos] == b'}' {
                        pos += 1;
                        break;
                    }
                    if b[pos] == b',' {
                        pos += 1;
                        continue;
                    }

                    let (key, next) = parse_str(b, pos)?;
                    pos = expect(b, next, b':')?;

                    if key == "l" {
                        let (val, next) = parse_str(b, pos)?;
                        let _ = route.line.push_str(val);
                        pos = next;
                    } else if key == "t" {
                        // Departures array: [{"e":N,"h":"HH:MM"}, ...]
                        pos = expect(b, pos, b'[')?;

                        loop {
                            pos = skip_ws(b, pos);
                            if pos >= b.len() || b[pos] == b']' {
                                pos += 1;
                                break;
                            }
                            if b[pos] == b',' {
                                pos += 1;
                                continue;
                            }

                            // Departure object: {"e":N,"h":"HH:MM"}
                            pos = expect(b, pos, b'{')?;
                            let mut dep = Departure::default();

                            loop {
                                pos = skip_ws(b, pos);
                                if pos >= b.len() || b[pos] == b'}' {
                                    pos += 1;
                                    break;
                                }
                                if b[pos] == b',' {
                                    pos += 1;
                                    continue;
                                }

                                let (dkey, next) = parse_str(b, pos)?;
                                pos = expect(b, next, b':')?;

                                if dkey == "e" {
                                    let (val, next) = parse_num(b, pos);
                                    dep.wait_minutes = val;
                                    pos = next;
                                } else if dkey == "h" {
                                    let (val, next) = parse_str(b, pos)?;
                                    let _ = dep.time_str.push_str(val);
                                    pos = next;
                                }
                            }

                            if route.departures.len() < 4 {
                                let _ = route.departures.push(dep);
                            }
                        }
                    }
                }

                if stop.routes.len() < 4 {
                    let _ = stop.routes.push(route);
                }
            }

            if result.stops.len() < 16 {
                let _ = result.stops.push(stop);
            }
        }
    }

    if result.stops.is_empty() {
        return Err("no stops parsed from JSON");
    }

    Ok(result)
}
