//! Process metrics: peak resident memory, CPU time, hardware description.

pub fn cores() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1)
}

pub fn cpu_name() -> String {
    std::env::var("PROCESSOR_IDENTIFIER")
        .ok()
        .or_else(|| {
            std::fs::read_to_string("/proc/cpuinfo")
                .ok()
                .and_then(|s| s.lines().find(|l| l.starts_with("model name")).map(|l| l.split(':').nth(1).unwrap_or("").trim().to_string()))
        })
        .unwrap_or_else(|| "unknown CPU".into())
}

#[cfg(windows)]
mod imp {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

    pub fn peak_rss_mb() -> f64 {
        let mut c: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
        c.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        let ok = unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) };
        if ok == 0 { 0.0 } else { c.PeakWorkingSetSize as f64 / 1_048_576.0 }
    }

    pub fn cpu_time() -> f64 {
        let z = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let (mut a, mut b, mut k, mut u) = (z, z, z, z);
        let ok = unsafe { GetProcessTimes(GetCurrentProcess(), &mut a, &mut b, &mut k, &mut u) };
        if ok == 0 {
            return 0.0;
        }
        let t = |f: FILETIME| ((f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64) as f64 / 1e7;
        t(k) + t(u)
    }

    pub fn total_ram_gb() -> f64 {
        let mut m: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
        m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        if unsafe { GlobalMemoryStatusEx(&mut m) } == 0 { 0.0 } else { m.ullTotalPhys as f64 / 1e9 }
    }
}

#[cfg(not(windows))]
mod imp {
    fn status_kb(key: &str) -> Option<f64> {
        std::fs::read_to_string("/proc/self/status").ok()?.lines().find(|l| l.starts_with(key))?.split_whitespace().nth(1)?.parse().ok()
    }
    pub fn peak_rss_mb() -> f64 {
        status_kb("VmHWM:").map(|kb| kb / 1024.0).unwrap_or(0.0)
    }
    pub fn cpu_time() -> f64 {
        // utime + stime from /proc/self/stat (clock ticks, assume 100 Hz)
        std::fs::read_to_string("/proc/self/stat")
            .ok()
            .and_then(|s| {
                let after = s.rsplit_once(')')?.1;
                let f: Vec<&str> = after.split_whitespace().collect();
                Some((f.get(11)?.parse::<f64>().ok()? + f.get(12)?.parse::<f64>().ok()?) / 100.0)
            })
            .unwrap_or(0.0)
    }
    pub fn total_ram_gb() -> f64 {
        std::fs::read_to_string("/proc/meminfo")
            .ok()
            .and_then(|s| s.lines().next()?.split_whitespace().nth(1)?.parse::<f64>().ok())
            .map(|kb| kb / 1e6)
            .unwrap_or(0.0)
    }
}

pub use imp::*;
