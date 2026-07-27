// Phase 2/3 foundation: wired into production event loop in Phase 3.
#![allow(dead_code)]

//! Deterministic runtime ports for the effect runner.
//!
//! These traits allow the runner to be driven in tests without real time,
//! randomness, or network I/O.

use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};

use crate::app::event::{AppEvent, ReadOperation};
use crate::effects::request::RequestContext;

// ---------------------------------------------------------------------------
// Transport ports
// ---------------------------------------------------------------------------

pub trait ApiTransport: Send + Sync {
    fn execute_read(
        &self,
        operation: ReadOperation,
        context: RequestContext,
    ) -> Pin<Box<dyn Future<Output = AppEvent> + Send + 'static>>;
}

pub trait SubscriptionTransport: Send + Sync {
    type Request: Send;

    fn connect(
        &self,
        request: Self::Request,
    ) -> Pin<Box<dyn Future<Output = AppEvent> + Send + '_>>;
}

pub trait SessionStore: Send + Sync {
    fn save(
        &self,
        snapshot: crate::user_config::SessionState,
    ) -> Pin<Box<dyn Future<Output = AppEvent> + Send + 'static>>;
}

// ---------------------------------------------------------------------------
// Deterministic infrastructure ports
// ---------------------------------------------------------------------------

pub trait Clock {
    fn now(&self) -> Instant;
}

pub trait Sleeper {
    fn sleep(&self, duration: Duration) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}

pub trait RandomSource {
    fn next_u64(&mut self) -> u64;
}

pub trait IdSource {
    fn next_id(&mut self) -> u64;
}

pub trait BackoffPolicy {
    fn delay(&self, attempt: u32, random: &mut dyn RandomSource) -> Duration;
}

// ---------------------------------------------------------------------------
// Test fakes
// ---------------------------------------------------------------------------

#[cfg(test)]
#[derive(Debug, Clone)]
pub struct FakeClock {
    now: Instant,
}

#[cfg(test)]
impl FakeClock {
    pub fn new(now: Instant) -> Self {
        Self { now }
    }
}

#[cfg(test)]
impl Clock for FakeClock {
    fn now(&self) -> Instant {
        self.now
    }
}

#[cfg(test)]
pub struct FakeRandomSource {
    values: Vec<u64>,
}

#[cfg(test)]
impl FakeRandomSource {
    pub fn new(values: Vec<u64>) -> Self {
        Self { values }
    }
}

#[cfg(test)]
impl RandomSource for FakeRandomSource {
    fn next_u64(&mut self) -> u64 {
        self.values.remove(0)
    }
}

#[cfg(test)]
pub struct FakeIdSource {
    value: u64,
}

#[cfg(test)]
impl FakeIdSource {
    pub fn new(value: u64) -> Self {
        Self { value }
    }
}

#[cfg(test)]
impl IdSource for FakeIdSource {
    fn next_id(&mut self) -> u64 {
        self.value += 1;
        self.value
    }
}

#[cfg(test)]
pub struct FixedBackoffPolicy {
    pub base: Duration,
}

#[cfg(test)]
impl BackoffPolicy for FixedBackoffPolicy {
    fn delay(&self, attempt: u32, random: &mut dyn RandomSource) -> Duration {
        self.base * attempt + Duration::from_millis(random.next_u64())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_ports_work_without_real_time_or_randomness() {
        let mut ids = FakeIdSource::new(40);
        let mut random = FakeRandomSource::new(vec![3]);
        let backoff = FixedBackoffPolicy {
            base: Duration::from_millis(100),
        };
        let clock = FakeClock::new(Instant::now());

        assert_eq!(ids.next_id(), 41);
        assert_eq!(random.next_u64(), 3);
        let mut random = FakeRandomSource::new(vec![3]);
        assert_eq!(backoff.delay(2, &mut random), Duration::from_millis(203));
        assert_eq!(clock.now(), clock.now());
    }
}
