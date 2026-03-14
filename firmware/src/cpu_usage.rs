use portable_atomic::{AtomicU32, Ordering};
use embassy_time::{Duration, Timer};
use embassy_executor::Spawner;

// Counters for idle loops
pub static CORE0_IDLE_COUNT: AtomicU32 = AtomicU32::new(0);
pub static CORE1_IDLE_COUNT: AtomicU32 = AtomicU32::new(0);

// Global values representing the CPU percentage (0-100)
pub static CORE0_USAGE_PCT: AtomicU32 = AtomicU32::new(0);
pub static CORE1_USAGE_PCT: AtomicU32 = AtomicU32::new(0);

// Baseline counts when the CPU is 100% idle (calibrated over 1 second)
pub static CORE0_MAX_IDLE: AtomicU32 = AtomicU32::new(100_000);
pub static CORE1_MAX_IDLE: AtomicU32 = AtomicU32::new(100_000);

// Spawns the monitor task on Core 0 that calculates the usage every second
pub fn spawn_cpu_monitor(spawner: Spawner) {
    spawner.spawn(cpu_monitor_task()).unwrap();
}

#[embassy_executor::task]
async fn cpu_monitor_task() {
    loop {
        // Wait 1 second
        Timer::after(Duration::from_secs(1)).await;

        // Read current idle counts and reset
        let c0_idle = CORE0_IDLE_COUNT.swap(0, Ordering::Relaxed);
        let c1_idle = CORE1_IDLE_COUNT.swap(0, Ordering::Relaxed);

        // Update max baselines if we see a higher idle count (system is more idle than before)
        let c0_max = CORE0_MAX_IDLE.load(Ordering::Relaxed);
        if c0_idle > c0_max {
            CORE0_MAX_IDLE.store(c0_idle, Ordering::Relaxed);
        } else {
            // Decay max slightly to adapt over time (e.g. 1% per second)
            CORE0_MAX_IDLE.store(c0_max.saturating_sub(c0_max / 100), Ordering::Relaxed);
        }

        let c1_max = CORE1_MAX_IDLE.load(Ordering::Relaxed);
        if c1_idle > c1_max {
            CORE1_MAX_IDLE.store(c1_idle, Ordering::Relaxed);
        } else {
            CORE1_MAX_IDLE.store(c1_max.saturating_sub(c1_max / 100), Ordering::Relaxed);
        }

        // Calculate usage percentages (inverted: 100% - idle%)
        let c0_max_curr = CORE0_MAX_IDLE.load(Ordering::Relaxed).max(1);
        let c1_max_curr = CORE1_MAX_IDLE.load(Ordering::Relaxed).max(1);

        let c0_idle_pct = (c0_idle.min(c0_max_curr) * 100) / c0_max_curr;
        let c1_idle_pct = (c1_idle.min(c1_max_curr) * 100) / c1_max_curr;

        CORE0_USAGE_PCT.store(100 - c0_idle_pct, Ordering::Relaxed);
        CORE1_USAGE_PCT.store(100 - c1_idle_pct, Ordering::Relaxed);
    }
}

#[embassy_executor::task]
pub async fn core0_idle_task() {
    loop {
        CORE0_IDLE_COUNT.fetch_add(1, Ordering::Relaxed);
        embassy_futures::yield_now().await;
    }
}

#[embassy_executor::task]
pub async fn core1_idle_task() {
    loop {
        CORE1_IDLE_COUNT.fetch_add(1, Ordering::Relaxed);
        embassy_futures::yield_now().await;
    }
}
