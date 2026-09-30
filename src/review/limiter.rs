//! One adaptive gate per client, shared across clones and every review stage.
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::Notify;

struct State {
    active: usize,
    limit: usize,
    healthy: usize,
    cooldown: Instant,
}
pub struct AdaptiveLimiter {
    cap: usize,
    state: Mutex<State>,
    changed: Notify,
}
pub struct Permit {
    gate: Arc<AdaptiveLimiter>,
}
impl AdaptiveLimiter {
    pub fn new(cap: usize) -> Arc<Self> {
        Arc::new(Self {
            cap: cap.max(1),
            state: Mutex::new(State {
                active: 0,
                limit: cap.clamp(1, 3),
                healthy: 0,
                cooldown: Instant::now(),
            }),
            changed: Notify::new(),
        })
    }
    pub async fn acquire(self: &Arc<Self>) -> Permit {
        loop {
            // Register before testing capacity; this avoids a lost wakeup.
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let delay = {
                let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                if state.cooldown <= Instant::now() && state.active < state.limit {
                    state.active += 1;
                    return Permit { gate: self.clone() };
                }
                state.cooldown.saturating_duration_since(Instant::now())
            };
            if delay.is_zero() {
                notified.await;
            } else {
                tokio::select! { _ = &mut notified => {}, _ = tokio::time::sleep(delay) => {} }
            }
        }
    }
    pub fn healthy(&self) {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        s.healthy += 1;
        if s.healthy >= 8 {
            s.limit = (s.limit + 1).min(self.cap);
            s.healthy = 0;
        }
        drop(s);
        self.changed.notify_waiters();
    }
    pub fn throttled(&self, pause: Duration) {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        s.limit = (s.limit / 2).max(1);
        s.healthy = 0;
        s.cooldown = s
            .cooldown
            .max(Instant::now() + pause.min(Duration::from_secs(300)));
        drop(s);
        self.changed.notify_waiters();
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        let mut s = self.gate.state.lock().unwrap_or_else(|e| e.into_inner());
        s.active -= 1;
        drop(s);
        self.gate.changed.notify_waiters();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn throttling_reduces_capacity_then_health_ramps_within_the_cap() {
        let gate = AdaptiveLimiter::new(4);
        let a = gate.acquire().await;
        let b = gate.acquire().await;
        let c = gate.acquire().await;
        assert!(
            tokio::time::timeout(Duration::from_millis(10), gate.acquire())
                .await
                .is_err()
        );
        gate.throttled(Duration::from_millis(20));
        drop(a);
        drop(b);
        assert!(
            tokio::time::timeout(Duration::from_millis(10), gate.acquire())
                .await
                .is_err()
        );
        drop(c);
        let permit = gate.acquire().await;
        assert_eq!(gate.state.lock().unwrap().limit, 1);
        for _ in 0..100 {
            gate.healthy();
        }
        assert_eq!(gate.state.lock().unwrap().limit, 4);
        drop(permit);
        assert_eq!(gate.state.lock().unwrap().active, 0);
    }
}
