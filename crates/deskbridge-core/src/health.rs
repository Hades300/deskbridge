use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthState {
    Starting,
    Connecting,
    Connected,
    Degraded,
    Disconnected,
}

#[derive(Debug, Clone)]
pub struct ConnectionHealth {
    state: HealthState,
    last_rx: Option<Instant>,
    consecutive_failures: u32,
    heartbeat_timeout: Duration,
}

impl ConnectionHealth {
    pub fn new(heartbeat_timeout: Duration) -> Self {
        Self {
            state: HealthState::Starting,
            last_rx: None,
            consecutive_failures: 0,
            heartbeat_timeout,
        }
    }

    pub fn state(&self) -> HealthState {
        self.state
    }

    pub fn mark_connecting(&mut self) {
        self.state = HealthState::Connecting;
    }

    pub fn mark_connected(&mut self) {
        self.state = HealthState::Connected;
        self.last_rx = Some(Instant::now());
        self.consecutive_failures = 0;
    }

    pub fn mark_rx(&mut self) {
        self.last_rx = Some(Instant::now());
        self.consecutive_failures = 0;
        self.state = HealthState::Connected;
    }

    pub fn mark_failure(&mut self) {
        self.consecutive_failures += 1;
        self.state = if self.consecutive_failures > 2 {
            HealthState::Disconnected
        } else {
            HealthState::Degraded
        };
    }

    pub fn is_stale(&self) -> bool {
        self.last_rx
            .map(|last| last.elapsed() > self.heartbeat_timeout)
            .unwrap_or(true)
    }

    pub fn consecutive_failures(&self) -> u32 {
        self.consecutive_failures
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failures_escalate_state() {
        let mut health = ConnectionHealth::new(Duration::from_secs(1));
        health.mark_failure();
        assert_eq!(health.state(), HealthState::Degraded);
        health.mark_failure();
        health.mark_failure();
        assert_eq!(health.state(), HealthState::Disconnected);
    }
}
