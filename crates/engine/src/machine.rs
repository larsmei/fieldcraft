//! The machine a session runs on: its physical memory, which bounds how far a fluid solve's
//! memory caps may be raised above their defaults ([`Limits::memory`](crate::Limits::memory)).

/// The share of physical memory a run with raised caps may take: half, leaving the rest to the
/// system, the application, its results and other programs.
const MEMORY_SHARE: u64 = 2;

/// The machine's physical memory in bytes (on Linux the root cgroup's limit when lower, so a
/// container's), or `None` where it cannot be read (the browser). Read once per process.
#[cfg(not(target_arch = "wasm32"))]
pub fn physical_memory() -> Option<u64> {
    use sysinfo::{MemoryRefreshKind, RefreshKind, System};
    static MEMORY: std::sync::OnceLock<Option<u64>> = std::sync::OnceLock::new();
    *MEMORY.get_or_init(|| {
        // An unsupported system is an error since sysinfo 0.40 (it had `IS_SUPPORTED_SYSTEM`).
        let system = System::new_with_specifics(RefreshKind::nothing().with_memory(MemoryRefreshKind::nothing().with_ram())).ok()?;
        let total = system.total_memory();
        let total = system.cgroup_limits().map_or(total, |c| if c.total_memory > 0 { total.min(c.total_memory) } else { total });
        (total > 0).then_some(total)
    })
}

/// In the browser the memory is unknown: the caps keep their defaults.
#[cfg(target_arch = "wasm32")]
pub fn physical_memory() -> Option<u64> {
    None
}

/// The bound on a run with raised caps: [`MEMORY_SHARE`] of the physical memory, or 0 where it
/// is unknown (the caps then cannot be raised).
pub fn memory_bound() -> u64 {
    physical_memory().map_or(0, |m| m / MEMORY_SHARE)
}

/// Peak resident memory and wall-clock phases of one analysis that ran in this process.
///
/// It describes the run, not the model, so it is not written into the results file.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RunCost {
    /// Peak resident set of this process while the step ran, when it could be read.
    pub ram_bytes: Option<u64>,
    /// Resolving the model, the mesh, assembly and boundary conditions.
    pub prepare_secs: f64,
    /// Factorization and the solve itself.
    pub solve_secs: f64,
    /// Stresses, fluxes, reactions and the result arrays after the unknowns are known.
    pub results_secs: f64,
}

impl RunCost {
    /// The three phases added.
    pub fn total_secs(self) -> f64 {
        self.prepare_secs + self.solve_secs + self.results_secs
    }
}

/// Samples this process's resident set while a solve runs, on its own thread.
///
/// [`RamWatch::peak`] is the largest sample, including one taken as the watch starts and one
/// as it stops. A sample of 0, or a system that cannot be read (the browser), is `None`.
pub struct RamWatch {
    #[cfg(not(target_arch = "wasm32"))]
    inner: Option<Watch>,
}

#[cfg(not(target_arch = "wasm32"))]
struct Watch {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    peak: std::sync::Arc<std::sync::atomic::AtomicU64>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl RamWatch {
    /// Start sampling. The watch stops when it is dropped or [`RamWatch::peak`] is called.
    pub fn start() -> Self {
        #[cfg(target_arch = "wasm32")]
        {
            RamWatch {}
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
            use std::sync::Arc;
            let stop = Arc::new(AtomicBool::new(false));
            let peak = Arc::new(AtomicU64::new(0));
            if let Some(rss) = current_rss() {
                peak.store(rss, Ordering::Relaxed);
            }
            let stop_t = Arc::clone(&stop);
            let peak_t = Arc::clone(&peak);
            let handle = std::thread::Builder::new().name("fieldcraft-rss".into()).spawn(move || sample_loop(&stop_t, &peak_t)).ok();
            RamWatch { inner: handle.map(|handle| Watch { stop, peak, handle: Some(handle) }) }
        }
    }

    /// Stop sampling and return the largest resident set seen, or `None` when none could be read.
    pub fn peak(mut self) -> Option<u64> {
        self.stop()
    }

    fn stop(&mut self) -> Option<u64> {
        #[cfg(target_arch = "wasm32")]
        {
            None
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            use std::sync::atomic::Ordering;
            let Some(inner) = self.inner.as_mut() else { return None };
            if let Some(rss) = current_rss() {
                inner.peak.fetch_max(rss, Ordering::Relaxed);
            }
            inner.stop.store(true, Ordering::Relaxed);
            if let Some(handle) = inner.handle.take() {
                let _ = handle.join();
            }
            let peak = inner.peak.load(Ordering::Relaxed);
            (peak > 0).then_some(peak)
        }
    }
}

impl Drop for RamWatch {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

/// Resident set of this process, in bytes.
#[cfg(not(target_arch = "wasm32"))]
fn current_rss() -> Option<u64> {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System};
    let mut system = System::new_with_specifics(RefreshKind::nothing()).ok()?;
    let pid = Pid::from_u32(std::process::id());
    let kind = ProcessRefreshKind::nothing().without_tasks().with_memory();
    system.refresh_processes_specifics(ProcessesToUpdate::Some(&[pid]), true, kind);
    let rss = system.process(pid)?.memory();
    (rss > 0).then_some(rss)
}

/// Sample about every 25 ms until `stop`, keeping the largest resident set.
#[cfg(not(target_arch = "wasm32"))]
fn sample_loop(stop: &std::sync::atomic::AtomicBool, peak: &std::sync::atomic::AtomicU64) {
    use std::sync::atomic::Ordering;
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System};
    let Ok(mut system) = System::new_with_specifics(RefreshKind::nothing()) else { return };
    let pid = Pid::from_u32(std::process::id());
    let kind = ProcessRefreshKind::nothing().without_tasks().with_memory();
    while !stop.load(Ordering::Relaxed) {
        system.refresh_processes_specifics(ProcessesToUpdate::Some(&[pid]), true, kind);
        if let Some(process) = system.process(pid) {
            let rss = process.memory();
            if rss > 0 {
                peak.fetch_max(rss, Ordering::Relaxed);
            }
        }
        for _ in 0..5 {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::RamWatch;

    #[test]
    fn a_watch_reads_the_process() {
        let peak = RamWatch::start().peak();
        assert!(peak.is_some_and(|b| b > 0), "{peak:?}");
    }
}
