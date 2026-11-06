use super::*;
use std::sync::mpsc::{self, Receiver};

pub(crate) struct NetworkSample {
    received: u64,
    transmitted: u64,
    at: Instant,
}

impl NetworkSample {
    fn new(received: u64, transmitted: u64, at: Instant) -> Self {
        Self {
            received,
            transmitted,
            at,
        }
    }

    fn update(&mut self, received: u64, transmitted: u64, at: Instant) -> (f64, f64) {
        let elapsed = at.saturating_duration_since(self.at).as_secs_f64();
        let rates = if elapsed > 0.0 {
            (
                received.saturating_sub(self.received) as f64 / elapsed,
                transmitted.saturating_sub(self.transmitted) as f64 / elapsed,
            )
        } else {
            (0.0, 0.0)
        };
        *self = Self::new(received, transmitted, at);
        rates
    }
}

pub(crate) struct Collector {
    sys: System,
    disks: Disks,
    nets: Networks,
    network: NetworkSample,
    cpu_name: String,
    cpu_cores: usize,
}

fn totals(nets: &Networks) -> (u64, u64) {
    (
        nets.values().map(|n| n.total_received()).sum(),
        nets.values().map(|n| n.total_transmitted()).sum(),
    )
}

impl Collector {
    pub(crate) fn new() -> Self {
        let sys = System::new_with_specifics(
            RefreshKind::nothing()
                .with_cpu(CpuRefreshKind::everything())
                .with_memory(MemoryRefreshKind::everything()),
        );
        let disks = Disks::new_with_refreshed_list();
        let nets = Networks::new_with_refreshed_list();
        let (rx, tx) = totals(&nets);
        let cpu_name = sys
            .cpus()
            .first()
            .map(|c| c.brand().to_string())
            .unwrap_or_default();
        let cpu_cores = sys.cpus().len();
        Self {
            sys,
            disks,
            nets,
            network: NetworkSample::new(rx, tx, Instant::now()),
            cpu_name,
            cpu_cores,
        }
    }

    pub(crate) fn collect(&mut self, stop: &AtomicBool) -> Metrics {
        self.sys.refresh_cpu_usage();
        self.sys.refresh_memory();
        self.disks.refresh(true);
        self.nets.refresh(true);
        let (rx, tx) = totals(&self.nets);
        // Timestamp the counter sample before doing any GPU work.
        let (down, up) = self.network.update(rx, tx, Instant::now());
        let gpus = get_gpu_info(stop);
        collect_metrics(
            &self.sys,
            &self.disks,
            gpus,
            &self.cpu_name,
            self.cpu_cores,
            down,
            up,
        )
    }
}

#[cfg(unix)]
pub(crate) struct Worker {
    pub(crate) receiver: Receiver<Metrics>,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

#[cfg(unix)]
impl Worker {
    pub(crate) fn start() -> Self {
        let (sender, receiver) = mpsc::sync_channel(1);
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let handle = thread::spawn(move || {
            let mut collector = Collector::new();
            thread::park_timeout(Duration::from_millis(500));
            while !worker_stop.load(Ordering::Relaxed) {
                let start = Instant::now();
                let sample = collector.collect(&worker_stop);
                if let Err(mpsc::TrySendError::Disconnected(_)) = sender.try_send(sample) {
                    break;
                }
                if !worker_stop.load(Ordering::Relaxed) {
                    thread::park_timeout(Duration::from_secs(1).saturating_sub(start.elapsed()));
                }
            }
        });
        Self {
            receiver,
            stop,
            handle: Some(handle),
        }
    }
}

#[cfg(unix)]
impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            handle.thread().unpark();
            // A system metrics syscall may block (e.g. an unresponsive mount).
            // Allow a running GPU probe to be killed/reaped, but bound shutdown.
            let deadline = Instant::now() + Duration::from_millis(250);
            while !handle.is_finished() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(5));
            }
            if handle.is_finished() {
                let _ = handle.join();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates_use_time_between_counter_samples() {
        let start = Instant::now();
        let mut sample = NetworkSample::new(100, 200, start);
        assert_eq!(
            sample.update(1100, 700, start + Duration::from_secs(1)),
            (1000.0, 500.0)
        );
        // A delayed probe or a resize must not reset the counter interval.
        assert_eq!(
            sample.update(4100, 2200, start + Duration::from_secs(4)),
            (1000.0, 500.0)
        );
    }

    #[test]
    fn counter_resets_and_zero_intervals_do_not_spike() {
        let start = Instant::now();
        let mut sample = NetworkSample::new(1000, 2000, start);
        assert_eq!(
            sample.update(0, 0, start + Duration::from_secs(1)),
            (0.0, 0.0)
        );
        assert_eq!(
            sample.update(1, 1, start + Duration::from_secs(1)),
            (0.0, 0.0)
        );
    }
}
