/*!
# cuda-backpressure

Flow control and backpressure for agent communication.

When a downstream agent is slow, upstream agents need to slow down
too. Backpressure prevents buffer overflows and memory exhaustion.

- Credit-based flow control
- Window-based flow control
- Signal levels (Green/Yellow/Red)
- Adaptive rate adjustment
- Queue depth monitoring
*/

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Signal { Green, Yellow, Red }

/// Credit-based flow control
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreditFlow {
    pub sender: String,
    pub receiver: String,
    pub credits: u32,
    pub max_credits: u32,
    pub credit_size: u32,  // bytes per credit
    pub total_sent: u64,
    pub total_blocked: u64,
    pub total_refilled: u64,
}

impl CreditFlow {
    pub fn new(sender: &str, receiver: &str, max_credits: u32, credit_size: u32) -> Self {
        CreditFlow { sender: sender.to_string(), receiver: receiver.to_string(), credits: max_credits, max_credits, credit_size, total_sent: 0, total_blocked: 0, total_refilled: 0 }
    }

    /// Try to send — consumes one credit
    pub fn try_send(&mut self) -> bool {
        if self.credits == 0 { self.total_blocked += 1; return false; }
        self.credits -= 1;
        self.total_sent += 1;
        true
    }

    /// Refill credits (receiver acknowledges)
    pub fn refill(&mut self, count: u32) {
        self.credits = (self.credits + count).min(self.max_credits);
        self.total_refilled += count as u64;
    }

    /// Full refill
    pub fn refill_all(&mut self) { self.refill(self.max_credits - self.credits); }

    /// Available credit ratio
    pub fn credit_ratio(&self) -> f64 { if self.max_credits == 0 { 0.0 } else { self.credits as f64 / self.max_credits as f64 } }

    pub fn signal(&self) -> Signal {
        let r = self.credit_ratio();
        if r > 0.5 { Signal::Green }
        else if r > 0.1 { Signal::Yellow }
        else { Signal::Red }
    }
}

/// Window-based flow control
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WindowFlow {
    pub window_size: u32,
    pub in_flight: u32,
    pub acked: u64,
    pub total_sent: u64,
}

impl WindowFlow {
    pub fn new(window_size: u32) -> Self { WindowFlow { window_size, in_flight: 0, acked: 0, total_sent: 0 } }

    pub fn can_send(&self) -> bool { self.in_flight < self.window_size }

    pub fn send(&mut self) -> bool {
        if !self.can_send() { return false; }
        self.in_flight += 1;
        self.total_sent += 1;
        true
    }

    pub fn ack(&mut self) {
        if self.in_flight > 0 { self.in_flight -= 1; self.acked += 1; }
    }

    pub fn window_utilization(&self) -> f64 { if self.window_size == 0 { return 0.0; } self.in_flight as f64 / self.window_size as f64 }
}

/// Adaptive rate controller
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AdaptiveController {
    pub current_rate: f64,
    pub min_rate: f64,
    pub max_rate: f64,
    pub target_latency_ms: f64,
    pub samples: Vec<f64>,
    pub max_samples: usize,
}

impl AdaptiveController {
    pub fn new(min_rate: f64, max_rate: f64, target_latency_ms: f64) -> Self {
        AdaptiveController { current_rate: max_rate, min_rate, max_rate, target_latency_ms, samples: vec![], max_samples: 50 }
    }

    /// Record observed latency and adjust rate
    pub fn observe(&mut self, latency_ms: f64) {
        self.samples.push(latency_ms);
        if self.samples.len() > self.max_samples { self.samples.remove(0); }

        let avg: f64 = self.samples.iter().sum::<f64>() / self.samples.len() as f64;
        let ratio = self.target_latency_ms / avg.max(1.0);

        // AIMD: Additive Increase, Multiplicative Decrease
        if ratio > 1.0 {
            // Latency below target — increase rate
            self.current_rate = (self.current_rate + (self.max_rate - self.current_rate) * 0.1).min(self.max_rate);
        } else {
            // Latency above target — decrease rate
            self.current_rate = (self.current_rate * ratio).max(self.min_rate);
        }
    }

    /// Current signal
    pub fn signal(&self) -> Signal {
        let r = (self.current_rate - self.min_rate) / (self.max_rate - self.min_rate).max(1.0);
        if r > 0.6 { Signal::Green }
        else if r > 0.2 { Signal::Yellow }
        else { Signal::Red }
    }
}

/// Queue monitor
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QueueMonitor {
    pub name: String,
    pub depth: usize,
    pub max_depth: usize,
    pub high_water: usize,
    pub low_water: usize,
    pub total_enqueued: u64,
    pub total_dropped: u64,
}

impl QueueMonitor {
    pub fn new(name: &str, max_depth: usize) -> Self {
        let hw = (max_depth as f64 * 0.8) as usize;
        let lw = (max_depth as f64 * 0.2) as usize;
        QueueMonitor { name: name.to_string(), depth: 0, max_depth, high_water: hw, low_water: lw, total_enqueued: 0, total_dropped: 0 }
    }

    pub fn enqueue(&mut self) -> bool {
        self.total_enqueued += 1;
        if self.depth >= self.max_depth { self.total_dropped += 1; return false; }
        self.depth += 1;
        true
    }

    pub fn dequeue(&mut self) -> bool {
        if self.depth == 0 { return false; }
        self.depth -= 1;
        true
    }

    pub fn signal(&self) -> Signal {
        if self.depth >= self.high_water { Signal::Red }
        else if self.depth >= self.low_water { Signal::Yellow }
        else { Signal::Green }
    }

    pub fn utilization(&self) -> f64 { if self.max_depth == 0 { return 1.0; } self.depth as f64 / self.max_depth as f64 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_credit_flow() {
        let mut cf = CreditFlow::new("a", "b", 3, 100);
        assert!(cf.try_send());
        assert!(cf.try_send());
        assert!(cf.try_send());
        assert!(!cf.try_send()); // no credits
        cf.refill(2);
        assert!(cf.try_send());
    }

    #[test]
    fn test_credit_signals() {
        let mut cf = CreditFlow::new("a", "b", 10, 100);
        assert_eq!(cf.signal(), Signal::Green);
        for _ in 0..9 { cf.try_send(); }
        assert_eq!(cf.signal(), Signal::Yellow);
    }

    #[test]
    fn test_window_flow() {
        let mut wf = WindowFlow::new(3);
        assert!(wf.send()); assert!(wf.send()); assert!(wf.send());
        assert!(!wf.send()); // window full
        wf.ack();
        assert!(wf.send());
    }

    #[test]
    fn test_adaptive_increase() {
        let mut ac = AdaptiveController::new(1.0, 100.0, 50.0);
        for _ in 0..10 { ac.observe(25.0); } // below target → increase
        assert!(ac.current_rate > 50.0);
    }

    #[test]
    fn test_adaptive_decrease() {
        let mut ac = AdaptiveController::new(1.0, 100.0, 50.0);
        for _ in 0..10 { ac.observe(200.0); } // above target → decrease
        assert!(ac.current_rate < 100.0);
    }

    #[test]
    fn test_queue_monitor() {
        let mut qm = QueueMonitor::new("q1", 10);
        for _ in 0..10 { qm.enqueue(); }
        assert!(!qm.enqueue()); // dropped
        for _ in 0..5 { qm.dequeue(); }
        assert_eq!(qm.signal(), Signal::Red);
    }

    #[test]
    fn test_queue_drops_tracked() {
        let mut qm = QueueMonitor::new("q", 2);
        qm.enqueue(); qm.enqueue();
        qm.enqueue(); qm.enqueue(); qm.enqueue();
        assert_eq!(qm.total_dropped, 3);
    }

    #[test]
    fn test_window_utilization() {
        let mut wf = WindowFlow::new(4);
        wf.send(); wf.send();
        assert!((wf.window_utilization() - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_refill_clamps() {
        let mut cf = CreditFlow::new("a", "b", 5, 100);
        cf.try_send(); cf.try_send();
        cf.refill(10); // should clamp to max
        assert_eq!(cf.credits, 5);
    }
}
