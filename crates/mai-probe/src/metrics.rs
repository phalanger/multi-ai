//! Host metrics via sysinfo. Byte rates are averaged over the time since
//! the previous sample; the first sample reports zero rates.

use mai_protocol::Metrics;
use sysinfo::{Disks, Networks, System};

pub struct MetricsSampler {
    sys: System,
    nets: Networks,
    disks: Disks,
    last_ms: Option<u64>,
}

impl Default for MetricsSampler {
    fn default() -> Self {
        Self::new()
    }
}

/// 1-minute load average; not available on Windows.
fn load1() -> Option<f32> {
    (!cfg!(windows)).then(|| System::load_average().one as f32)
}

impl MetricsSampler {
    pub fn new() -> Self {
        let mut sys = System::new();
        sys.refresh_cpu_usage();
        sys.refresh_memory();
        Self {
            sys,
            nets: Networks::new_with_refreshed_list(),
            disks: Disks::new_with_refreshed_list(),
            last_ms: None,
        }
    }

    pub fn sample(&mut self, now_ms: u64) -> Metrics {
        self.sys.refresh_cpu_usage();
        self.sys.refresh_memory();
        self.nets.refresh(true);
        self.disks.refresh(true);
        let elapsed_ms = self
            .last_ms
            .map(|t| now_ms.saturating_sub(t))
            .filter(|d| *d > 0);
        self.last_ms = Some(now_ms);
        let rate = |bytes: u64| elapsed_ms.map_or(0, |ms| bytes.saturating_mul(1000) / ms);
        let (rx, tx) = self.nets.iter().fold((0, 0), |(r, t), (_, d)| {
            (r + d.received(), t + d.transmitted())
        });
        let (rd, wr) = self.disks.iter().fold((0, 0), |(r, w), d| {
            let u = d.usage();
            (r + u.read_bytes, w + u.written_bytes)
        });
        Metrics {
            cpu_pct: self.sys.global_cpu_usage(),
            mem_used: self.sys.used_memory(),
            mem_total: self.sys.total_memory(),
            disk_read_bps: rate(rd),
            disk_write_bps: rate(wr),
            net_rx_bps: rate(rx),
            net_tx_bps: rate(tx),
            load1: load1(),
            ts_ms: now_ms,
        }
    }
}
