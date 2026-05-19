use std::time::{Duration, Instant};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

#[derive(Debug, Clone, Default)]
pub struct ProcStats {
    pub cpu_percent: f32,
    pub rss_bytes: u64,
}

#[derive(Debug, Clone, Default)]
pub struct Stats {
    pub self_proc: ProcStats,
    pub mpv: ProcStats,
}

impl Stats {
    pub fn total_cpu(&self) -> f32 {
        self.self_proc.cpu_percent + self.mpv.cpu_percent
    }
    pub fn total_rss(&self) -> u64 {
        self.self_proc.rss_bytes + self.mpv.rss_bytes
    }
}

pub struct StatsSampler {
    sys: System,
    self_pid: Pid,
    mpv_pid: Pid,
    last: Instant,
    min_interval: Duration,
    cached: Stats,
}

impl StatsSampler {
    pub fn new(mpv_pid: u32) -> Self {
        let mut sys = System::new();
        let self_pid = Pid::from_u32(std::process::id());
        let mpv_pid = Pid::from_u32(mpv_pid);
        // Prime CPU counters: sysinfo needs two samples to compute CPU%.
        sys.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[self_pid, mpv_pid]),
            true,
            ProcessRefreshKind::new().with_cpu().with_memory(),
        );
        Self {
            sys,
            self_pid,
            mpv_pid,
            last: Instant::now(),
            // sysinfo recommends >= ~MINIMUM_CPU_UPDATE_INTERVAL (200ms typical)
            min_interval: Duration::from_millis(500),
            cached: Stats::default(),
        }
    }

    pub fn sample(&mut self) -> Stats {
        if self.last.elapsed() < self.min_interval {
            return self.cached.clone();
        }
        self.last = Instant::now();
        self.sys.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[self.self_pid, self.mpv_pid]),
            true,
            ProcessRefreshKind::new().with_cpu().with_memory(),
        );

        let read = |pid: Pid| -> ProcStats {
            match self.sys.process(pid) {
                Some(p) => ProcStats {
                    cpu_percent: p.cpu_usage(),
                    rss_bytes: p.memory(),
                },
                None => ProcStats::default(),
            }
        };
        self.cached = Stats {
            self_proc: read(self.self_pid),
            mpv: read(self.mpv_pid),
        };
        self.cached.clone()
    }
}

pub fn fmt_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;
    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.0} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}
