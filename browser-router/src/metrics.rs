use emissary_core::runtime::{Counter, Gauge, Histogram, MetricsHandle};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

#[derive(Clone, Debug, Default)]
pub struct MetricsSnapshot {
    pub counters: BTreeMap<&'static str, u64>,
    pub gauges: BTreeMap<&'static str, u64>,
    pub observations: BTreeMap<&'static str, u64>,
}

impl MetricsSnapshot {
    pub fn counter(&self, name: &str) -> u64 {
        self.counters.get(name).copied().unwrap_or(0)
    }
    pub fn gauge(&self, name: &str) -> u64 {
        self.gauges.get(name).copied().unwrap_or(0)
    }
}

thread_local! {
    static CURRENT: RefCell<Arc<Mutex<MetricsSnapshot>>> = RefCell::new(Arc::new(Mutex::new(MetricsSnapshot::default())));
}

#[derive(Clone)]
pub struct BrowserMetrics(Arc<Mutex<MetricsSnapshot>>);
pub struct Metric {
    state: Arc<Mutex<MetricsSnapshot>>,
    name: &'static str,
}

pub(crate) fn register() -> BrowserMetrics {
    let state = Arc::new(Mutex::new(MetricsSnapshot::default()));
    CURRENT.with(|current| *current.borrow_mut() = state.clone());
    BrowserMetrics(state)
}

pub fn metrics_snapshot() -> MetricsSnapshot {
    CURRENT.with(|current| current.borrow().lock().unwrap().clone())
}

impl Counter for Metric {
    fn increment(&mut self, value: usize) {
        let mut state = self.state.lock().unwrap();
        let count = state.counters.entry(self.name).or_default();
        *count = count.saturating_add(value as u64);
    }
    fn increment_with_label(&mut self, value: usize, _: &'static str, _: &'static str) {
        Counter::increment(self, value);
    }
}
impl Gauge for Metric {
    fn increment(&mut self, value: usize) {
        let mut state = self.state.lock().unwrap();
        let count = state.gauges.entry(self.name).or_default();
        *count = count.saturating_add(value as u64);
    }
    fn decrement(&mut self, value: usize) {
        let mut state = self.state.lock().unwrap();
        let count = state.gauges.entry(self.name).or_default();
        *count = count.saturating_sub(value as u64);
    }
}
impl Histogram for Metric {
    fn record(&mut self, _: f64) {
        let mut state = self.state.lock().unwrap();
        let count = state.observations.entry(self.name).or_default();
        *count = count.saturating_add(1);
    }
}
impl MetricsHandle for BrowserMetrics {
    fn counter(&self, name: &'static str) -> impl Counter {
        Metric {
            state: self.0.clone(),
            name,
        }
    }
    fn gauge(&self, name: &'static str) -> impl Gauge {
        Metric {
            state: self.0.clone(),
            name,
        }
    }
    fn histogram(&self, name: &'static str) -> impl Histogram {
        Metric {
            state: self.0.clone(),
            name,
        }
    }
}
