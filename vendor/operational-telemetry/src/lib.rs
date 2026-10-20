//! Fixed operational stages shared by independent services. No dynamic label API,
//! HTTP, database models, identities, URLs or credentials enter this library.
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
    time::{Duration, Instant},
};

/// Physical deployment unit; names are a closed label vocabulary.
#[derive(Clone, Copy)]
pub enum Service {
    Platform,
    WorkOrders,
    Stores,
    Monitor,
}
impl Service {
    fn name(self) -> &'static str {
        match self {
            Self::Platform => "platform",
            Self::WorkOrders => "work-orders",
            Self::Stores => "stores",
            Self::Monitor => "monitor",
        }
    }
}
/// Closed stage vocabulary. Context labels distinguish the two real consumers.
#[derive(Clone, Copy)]
#[repr(usize)]
pub enum Stage {
    Request,
    Authorization,
    PlatformHttp,
    BusinessSql,
    PoolAcquire,
    DbLock,
    PasswordQueue,
    PasswordExecution,
    ContextWorkOrders,
    ContextStores,
    ContextOther,
    Entry,
    Ready,
    Mount,
}
const STAGES: [Stage; 14] = [
    Stage::Request,
    Stage::Authorization,
    Stage::PlatformHttp,
    Stage::BusinessSql,
    Stage::PoolAcquire,
    Stage::DbLock,
    Stage::PasswordQueue,
    Stage::PasswordExecution,
    Stage::ContextWorkOrders,
    Stage::ContextStores,
    Stage::ContextOther,
    Stage::Entry,
    Stage::Ready,
    Stage::Mount,
];
impl Stage {
    fn name(self) -> &'static str {
        match self {
            Self::Request => "request",
            Self::Authorization => "authorization",
            Self::PlatformHttp => "platform_http",
            Self::BusinessSql => "business_sql",
            Self::PoolAcquire => "pool_acquire",
            Self::DbLock => "db_lock",
            Self::PasswordQueue => "password_queue",
            Self::PasswordExecution => "password_execution",
            Self::ContextWorkOrders => "context_work_orders",
            Self::ContextStores => "context_stores",
            Self::ContextOther => "context_other",
            Self::Entry => "entry",
            Self::Ready => "ready",
            Self::Mount => "mount",
        }
    }
}
/// Closed result vocabulary. Cancellation is recorded when a timer is dropped.
#[derive(Clone, Copy)]
#[repr(usize)]
pub enum Outcome {
    Ok,
    Denied,
    Timeout,
    Dependency,
    Saturated,
    Error,
    Cancelled,
}
const OUTCOMES: [Outcome; 7] = [
    Outcome::Ok,
    Outcome::Denied,
    Outcome::Timeout,
    Outcome::Dependency,
    Outcome::Saturated,
    Outcome::Error,
    Outcome::Cancelled,
];
impl Outcome {
    fn name(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Denied => "denied",
            Self::Timeout => "timeout",
            Self::Dependency => "dependency",
            Self::Saturated => "saturated",
            Self::Error => "error",
            Self::Cancelled => "cancelled",
        }
    }
    /// Classify HTTP status without accepting a route or arbitrary error text.
    pub fn http(status: u16) -> Self {
        match status {
            200..=399 => Self::Ok,
            429 => Self::Saturated,
            503 => Self::Dependency,
            408 | 504 => Self::Timeout,
            500..=599 => Self::Error,
            _ => Self::Denied,
        }
    }
}
const BOUNDS: [f64; 12] = [
    0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 20.0,
];
#[derive(Default)]
struct Cell {
    count: u64,
    sum: f64,
    buckets: [u64; 12],
}
/// Process-local registry. Memory and possible series are bounded by enums.
pub struct Registry {
    service: Service,
    cells: Mutex<[Cell; 98]>,
    inflight: [AtomicU64; 14],
}
impl Registry {
    /// Create an empty registry; callers normally keep one per deployment unit.
    pub fn new(service: Service) -> Self {
        Self {
            service,
            cells: Mutex::new(std::array::from_fn(|_| Cell::default())),
            inflight: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }
    /// Start an in-flight observation. Dropping without `finish` means cancelled.
    pub fn timer(&self, stage: Stage) -> Timer<'_> {
        self.inflight[stage as usize].fetch_add(1, Ordering::Relaxed);
        Timer {
            registry: self,
            stage,
            start: Instant::now(),
            outcome: Outcome::Cancelled,
        }
    }
    /// Record a completed observation, for trusted browser duration hints too.
    pub fn record(&self, stage: Stage, outcome: Outcome, duration: Duration) {
        let mut cells = self.cells.lock().unwrap_or_else(|e| e.into_inner());
        let cell = &mut cells[stage as usize * 7 + outcome as usize];
        let seconds = duration.as_secs_f64().min(86400.0);
        cell.count = cell.count.saturating_add(1);
        cell.sum += seconds;
        for (i, bound) in BOUNDS.iter().enumerate() {
            if seconds <= *bound {
                cell.buckets[i] = cell.buckets[i].saturating_add(1)
            }
        }
    }
    /// Render Prometheus text. Only closed labels are representable.
    pub fn render(&self) -> String {
        let cells = self.cells.lock().unwrap_or_else(|e| e.into_inner());
        let mut out = String::from(
            "# TYPE saas_stage_inflight gauge\n# TYPE saas_stage_duration_seconds histogram\n",
        );
        for stage in STAGES {
            out.push_str(&format!(
                "saas_stage_inflight{{service=\"{}\",stage=\"{}\"}} {}\n",
                self.service.name(),
                stage.name(),
                self.inflight[stage as usize].load(Ordering::Relaxed)
            ));
            for outcome in OUTCOMES {
                let c = &cells[stage as usize * 7 + outcome as usize];
                if c.count == 0 {
                    continue;
                }
                let labels = format!(
                    "service=\"{}\",stage=\"{}\",outcome=\"{}\"",
                    self.service.name(),
                    stage.name(),
                    outcome.name()
                );
                for (i, bound) in BOUNDS.iter().enumerate() {
                    out.push_str(&format!(
                        "saas_stage_duration_seconds_bucket{{{labels},le=\"{bound}\"}} {}\n",
                        c.buckets[i]
                    ));
                }
                out.push_str(&format!("saas_stage_duration_seconds_bucket{{{labels},le=\"+Inf\"}} {}\nsaas_stage_duration_seconds_sum{{{labels}}} {}\nsaas_stage_duration_seconds_count{{{labels}}} {}\n",c.count,c.sum,c.count));
            }
        }
        out
    }
}
/// An observation owns no resource permits and never changes application work.
pub struct Timer<'a> {
    registry: &'a Registry,
    stage: Stage,
    start: Instant,
    outcome: Outcome,
}
impl Timer<'_> {
    /// Set a known failure before an early return; cancellation remains default.
    pub fn outcome(&mut self, outcome: Outcome) {
        self.outcome = outcome;
    }
    /// Complete exactly once; drop records elapsed time and releases the gauge.
    pub fn finish(mut self, outcome: Outcome) {
        self.outcome = outcome;
    }
}
impl Drop for Timer<'_> {
    fn drop(&mut self) {
        self.registry
            .record(self.stage, self.outcome, self.start.elapsed());
        self.registry.inflight[self.stage as usize].fetch_sub(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancellation_and_inflight_are_observed() {
        let r = Registry::new(Service::Platform);
        let timer = r.timer(Stage::PasswordExecution);
        assert!(r.render().contains("stage=\"password_execution\"} 1"));
        drop(timer);
        let text = r.render();
        assert!(text.contains("outcome=\"cancelled\""));
        assert!(text.contains("stage=\"password_execution\"} 0"));
    }
    #[test]
    fn histogram_buckets_are_cumulative_and_fixed() {
        let r = Registry::new(Service::Stores);
        r.record(
            Stage::Authorization,
            Outcome::Denied,
            Duration::from_millis(50),
        );
        let text = r.render();
        assert!(text.contains("le=\"0.025\"} 0"));
        assert!(text.contains("le=\"0.05\"} 1"));
        assert!(text.contains("le=\"+Inf\"} 1"));
        assert!(!text.contains("url="));
    }
    #[test]
    fn completion_is_not_cancellation() {
        let r = Registry::new(Service::WorkOrders);
        r.timer(Stage::Request).finish(Outcome::http(503));
        let text = r.render();
        assert!(text.contains("outcome=\"dependency\""));
        assert!(!text.contains("outcome=\"cancelled\""));
    }
}
