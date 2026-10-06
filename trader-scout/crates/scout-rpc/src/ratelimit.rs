//! Client-side rate limiter: a token bucket in front of every HTTP attempt
//! (AGENTS.md invariants #13 and #19: respect provider quota, bounded
//! queues).
//!
//! - One limiter per endpoint, shared (`Arc`) by every clone of a client and
//!   every concurrent task. Waiters queue FIFO behind one async mutex and
//!   sleep exactly until their tokens exist (no polling loop). The queue is
//!   bounded: more than `max_waiters` simultaneous waiters is a typed error.
//! - Units are abstract: 1 unit per request by default, or a per-method
//!   weight (compute units) supplied by the caller. The rate is stored in
//!   milli-units per second and all math is integer (workspace
//!   `float_arithmetic = deny`).
//! - AIMD-style back-off: [`RateLimiter::on_rate_limited`] halves the rate
//!   (floor [`MIN_RATE_MILLI`]) when a provider answers 429 without
//!   `Retry-After`; halvings are debounced to one per second so a burst of
//!   concurrent 429s counts once. After [`RECOVER_STEP`] without a 429 the
//!   rate grows back additively by 1/[`RECOVER_DIVISOR`] of the initial rate
//!   per step, up to the initial rate (a burst of 429s must not leave a long
//!   run crawling at the floor for hours; live 2026-10-05: 250 -> 7.8 CU/s
//!   made one `eth_getBlockReceipts` wait over a minute). A single sleep is
//!   capped at one step so a recovered rate takes effect promptly.
//! - Time comes from `tokio::time` so tests run with paused time.

use std::fmt;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::time::Instant;

/// Lowest rate the limiter backs off to: 0.1 units per second.
pub const MIN_RATE_MILLI: u64 = 100;
/// Default bound of simultaneously waiting callers.
pub const DEFAULT_MAX_WAITERS: usize = 4_096;
const HALVE_DEBOUNCE: Duration = Duration::from_secs(1);
/// Quiet time (no 429) after which the rate grows by one recovery step.
pub const RECOVER_STEP: Duration = Duration::from_secs(10);
/// One recovery step adds `initial / RECOVER_DIVISOR` to the rate.
pub const RECOVER_DIVISOR: u64 = 8;

/// More callers are waiting for the limiter than its configured bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimiterSaturated {
    pub max_waiters: usize,
}

impl fmt::Display for RateLimiterSaturated {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "rate limiter queue is full ({} waiting callers)",
            self.max_waiters
        )
    }
}

impl std::error::Error for RateLimiterSaturated {}

/// Snapshot of a limiter's counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimiterStats {
    pub initial_rate_milli: u64,
    pub rate_milli: u64,
    pub halvings: u64,
    /// Recovery steps taken after halvings.
    pub recoveries: u64,
    /// Granted acquisitions.
    pub acquired: u64,
    /// Total time callers slept waiting for tokens.
    pub waited_ms: u64,
}

/// Called as `(old_rate_milli, new_rate_milli)` after each halving
/// (`new < old`) and once when recovery restores the initial rate
/// (`new > old`).
pub type HalvingHook = Arc<dyn Fn(u64, u64) + Send + Sync>;

#[derive(Debug)]
struct State {
    /// Signed: a request costing more than the bucket goes into debt.
    tokens_micro: i128,
    last: Instant,
    rate_milli: u64,
    last_halved: Option<Instant>,
    /// Start of the current quiet period counted towards recovery.
    recover_from: Instant,
    /// Lowest rate since the last full recovery (reported as `old` in the
    /// restore notice).
    lowest_milli: u64,
    /// Bucket size (micro-units); see [`RateLimiter::retune`].
    burst_micro: i128,
}

/// Token-bucket limiter, see the module docs.
pub struct RateLimiter {
    state: Mutex<State>,
    turn: tokio::sync::Mutex<()>,
    waiters: AtomicUsize,
    max_waiters: usize,
    initial_rate_milli: AtomicU64,
    halvings: AtomicU64,
    recoveries: AtomicU64,
    acquired: AtomicU64,
    waited_ms: AtomicU64,
    hook: Option<HalvingHook>,
}

impl fmt::Debug for RateLimiter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RateLimiter")
            .field("stats", &self.stats())
            .field("max_waiters", &self.max_waiters)
            .finish_non_exhaustive()
    }
}

struct WaiterGuard<'a>(&'a AtomicUsize);

impl Drop for WaiterGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

fn div(a: u128, b: u128) -> u128 {
    a.checked_div(b).unwrap_or(0)
}

impl RateLimiter {
    /// `units_per_sec` sustained rate (>= 1), `burst_units` bucket size
    /// (>= 1; the bucket starts full).
    #[must_use]
    pub fn new(units_per_sec: u64, burst_units: u64) -> Self {
        let rate_milli = units_per_sec.max(1).saturating_mul(1_000);
        let burst_micro = i128::from(burst_units.max(1)) * 1_000_000;
        Self {
            state: Mutex::new(State {
                tokens_micro: burst_micro,
                last: Instant::now(),
                rate_milli,
                last_halved: None,
                recover_from: Instant::now(),
                lowest_milli: rate_milli,
                burst_micro,
            }),
            turn: tokio::sync::Mutex::new(()),
            waiters: AtomicUsize::new(0),
            max_waiters: DEFAULT_MAX_WAITERS,
            initial_rate_milli: AtomicU64::new(rate_milli),
            halvings: AtomicU64::new(0),
            recoveries: AtomicU64::new(0),
            acquired: AtomicU64::new(0),
            waited_ms: AtomicU64::new(0),
            hook: None,
        }
    }

    /// Bound of simultaneously waiting callers (>= 1).
    #[must_use]
    pub fn with_max_waiters(mut self, max_waiters: usize) -> Self {
        self.max_waiters = max_waiters.max(1);
        self
    }

    /// Set a new sustained rate and bucket size (e.g. once the provider's
    /// plan is known). Resets the recovery target to the new rate; the bucket
    /// starts full at the new size.
    pub fn retune(&self, units_per_sec: u64, burst_units: u64) {
        let rate_milli = units_per_sec.max(1).saturating_mul(1_000);
        let burst_micro = i128::from(burst_units.max(1)) * 1_000_000;
        if let Ok(mut s) = self.state.lock() {
            s.rate_milli = rate_milli;
            s.lowest_milli = rate_milli;
            s.burst_micro = burst_micro;
            s.tokens_micro = burst_micro;
            s.last = Instant::now();
            s.recover_from = Instant::now();
        }
        self.initial_rate_milli.store(rate_milli, Ordering::Release);
    }

    /// Report each halving (the CLIs print it on stderr).
    #[must_use]
    pub fn with_halving_hook(mut self, hook: HalvingHook) -> Self {
        self.hook = Some(hook);
        self
    }

    #[must_use]
    pub fn stats(&self) -> RateLimiterStats {
        let rate_milli = self.state.lock().map_or(0, |s| s.rate_milli);
        RateLimiterStats {
            initial_rate_milli: self.initial_rate_milli.load(Ordering::Acquire),
            rate_milli,
            halvings: self.halvings.load(Ordering::Acquire),
            recoveries: self.recoveries.load(Ordering::Acquire),
            acquired: self.acquired.load(Ordering::Acquire),
            waited_ms: self.waited_ms.load(Ordering::Acquire),
        }
    }

    /// Accrue tokens since the last call, then apply any recovery steps that
    /// became due. Returns `Some((lowest, initial))` when this call restored
    /// the initial rate (the caller reports it outside the lock).
    fn refill(&self, s: &mut State) -> Option<(u64, u64)> {
        let now = Instant::now();
        let elapsed_us = now.saturating_duration_since(s.last).as_micros();
        s.last = now;
        let add =
            i128::try_from(div(elapsed_us * u128::from(s.rate_milli), 1_000)).unwrap_or(i128::MAX);
        s.tokens_micro = s.tokens_micro.saturating_add(add).min(s.burst_micro);
        if s.rate_milli >= self.initial_rate_milli.load(Ordering::Acquire) {
            return None;
        }
        let quiet = now.saturating_duration_since(s.recover_from);
        let steps = div(quiet.as_millis(), RECOVER_STEP.as_millis());
        if steps == 0 {
            return None;
        }
        let step = self
            .initial_rate_milli
            .load(Ordering::Acquire)
            .checked_div(RECOVER_DIVISOR)
            .unwrap_or(0)
            .max(1);
        let missing = self.initial_rate_milli.load(Ordering::Acquire) - s.rate_milli;
        let needed = missing.div_ceil(step);
        let steps64 = u64::try_from(steps).unwrap_or(u64::MAX).min(needed);
        s.rate_milli = s
            .rate_milli
            .saturating_add(step.saturating_mul(steps64))
            .min(self.initial_rate_milli.load(Ordering::Acquire));
        s.recover_from = u32::try_from(steps)
            .ok()
            .and_then(|n| RECOVER_STEP.checked_mul(n))
            .and_then(|d| s.recover_from.checked_add(d))
            .unwrap_or(now);
        self.recoveries.fetch_add(steps64, Ordering::AcqRel);
        (s.rate_milli == self.initial_rate_milli.load(Ordering::Acquire)).then(|| {
            let lowest = s.lowest_milli;
            s.lowest_milli = self.initial_rate_milli.load(Ordering::Acquire);
            (lowest, self.initial_rate_milli.load(Ordering::Acquire))
        })
    }

    /// Wait until `cost_units` tokens are available and take them. A cost
    /// above the bucket size waits for a FULL bucket and then takes the whole
    /// cost, leaving the bucket in debt that later callers pay off by waiting
    /// (a heavy method is slow, never stuck, and the sustained rate holds).
    ///
    /// # Errors
    /// [`RateLimiterSaturated`] when the waiter bound is exceeded.
    pub async fn acquire(&self, cost_units: u64) -> Result<(), RateLimiterSaturated> {
        if self.waiters.fetch_add(1, Ordering::AcqRel) >= self.max_waiters {
            self.waiters.fetch_sub(1, Ordering::AcqRel);
            return Err(RateLimiterSaturated {
                max_waiters: self.max_waiters,
            });
        }
        let _waiting = WaiterGuard(&self.waiters);
        let cost = i128::from(cost_units.max(1)) * 1_000_000;
        // FIFO: one caller at a time sleeps for its tokens.
        let _turn = self.turn.lock().await;
        loop {
            let (wait, restored) = {
                let Ok(mut s) = self.state.lock() else {
                    // Poisoned: never block the run on a broken limiter.
                    break;
                };
                let restored = self.refill(&mut s);
                let need_tokens = cost.min(s.burst_micro);
                let wait = if s.tokens_micro >= need_tokens {
                    s.tokens_micro = s.tokens_micro.saturating_sub(cost);
                    None
                } else {
                    let need = u128::try_from(need_tokens - s.tokens_micro).unwrap_or(0);
                    let us = div(
                        need * 1_000 + u128::from(s.rate_milli) - 1,
                        u128::from(s.rate_milli),
                    );
                    // Capped: re-check after a recovery step instead of
                    // sleeping out a wait computed at a floor rate.
                    Some(
                        Duration::from_micros(u64::try_from(us.max(1)).unwrap_or(u64::MAX))
                            .min(RECOVER_STEP),
                    )
                };
                (wait, restored)
            };
            if let (Some((old, new)), Some(h)) = (restored, &self.hook) {
                h(old, new);
            }
            match wait {
                None => break,
                Some(d) => {
                    tokio::time::sleep(d).await;
                    self.waited_ms.fetch_add(
                        u64::try_from(d.as_millis()).unwrap_or(u64::MAX),
                        Ordering::AcqRel,
                    );
                }
            }
        }
        self.acquired.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }

    /// A provider answered 429 without `Retry-After`: halve the rate (once
    /// per second at most) and empty the bucket. Returns the new rate in
    /// milli-units/s when it was lowered.
    pub fn on_rate_limited(&self) -> Option<u64> {
        let (old, new) = {
            let mut s = self.state.lock().ok()?;
            let now = Instant::now();
            s.tokens_micro = 0;
            s.last = now;
            // Any 429 (debounced or not) restarts the quiet period.
            s.recover_from = now;
            if s.last_halved
                .is_some_and(|t| now.saturating_duration_since(t) < HALVE_DEBOUNCE)
            {
                return None;
            }
            let old = s.rate_milli;
            let new = old.checked_div(2).unwrap_or(old).max(MIN_RATE_MILLI);
            if new >= old {
                return None;
            }
            s.rate_milli = new;
            s.last_halved = Some(now);
            s.lowest_milli = s.lowest_milli.min(new);
            (old, new)
        };
        self.halvings.fetch_add(1, Ordering::AcqRel);
        if let Some(h) = &self.hook {
            h(old, new);
        }
        Some(new)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn sustained_rate_is_respected_with_paused_time() {
        let l = RateLimiter::new(2, 1); // 2 units/s, bucket of 1
        let t0 = Instant::now();
        for _ in 0..5 {
            l.acquire(1).await.unwrap();
        }
        // first is free (full bucket), the other 4 take 500 ms each
        let el = Instant::now() - t0;
        assert!(el >= Duration::from_millis(2_000), "{el:?}");
        assert!(el < Duration::from_millis(2_100), "{el:?}");
        assert_eq!(l.stats().acquired, 5);
    }

    #[tokio::test(start_paused = true)]
    async fn weights_cost_proportionally_and_oversize_goes_into_debt() {
        let l = RateLimiter::new(100, 100);
        let t0 = Instant::now();
        l.acquire(100).await.unwrap(); // drains the bucket
        l.acquire(50).await.unwrap(); // 0.5 s
        l.acquire(10_000).await.unwrap(); // waits for a full bucket (1 s), then debt
        let el = Instant::now() - t0;
        assert!(el >= Duration::from_millis(1_500) && el < Duration::from_millis(1_600));
    }

    #[tokio::test(start_paused = true)]
    async fn request_costing_more_than_the_bucket_is_admitted_after_a_wait() {
        // 250 units/s, bucket 250: a 500-unit call waits for a full bucket
        // (1 s after a drain) and is then admitted; its 250-unit debt delays
        // the next caller.
        let l = RateLimiter::new(250, 250);
        l.acquire(250).await.unwrap(); // drain
        let t0 = Instant::now();
        l.acquire(500).await.unwrap();
        let first = Instant::now() - t0;
        assert!(first >= Duration::from_millis(1_000) && first < Duration::from_millis(1_100));
        // debt: 500 - 250 = 250 below zero -> 2 s until the bucket is full
        // again, i.e. the sustained rate (500 units per 2 s) is respected.
        l.acquire(250).await.unwrap();
        let total = Instant::now() - t0;
        assert!(total >= Duration::from_millis(3_000), "{total:?}");
        assert!(total < Duration::from_millis(3_200), "{total:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn concurrent_callers_share_one_rate() {
        let l = Arc::new(RateLimiter::new(10, 1));
        let t0 = Instant::now();
        let mut hs = Vec::new();
        for _ in 0..11 {
            let l = l.clone();
            hs.push(tokio::spawn(async move { l.acquire(1).await.unwrap() }));
        }
        for h in hs {
            h.await.unwrap();
        }
        // 11 tokens at 10/s with one free: 1.0 s
        let el = Instant::now() - t0;
        assert!(el >= Duration::from_millis(1_000) && el < Duration::from_millis(1_100));
    }

    #[tokio::test(start_paused = true)]
    async fn halving_is_debounced_floored_and_reported() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s2 = seen.clone();
        let l = RateLimiter::new(8, 1).with_halving_hook(Arc::new(move |a, b| {
            if let Ok(mut v) = s2.lock() {
                v.push((a, b));
            }
        }));
        assert_eq!(l.on_rate_limited(), Some(4_000));
        assert_eq!(l.on_rate_limited(), None, "same instant: debounced");
        tokio::time::advance(Duration::from_millis(1_100)).await;
        assert_eq!(l.on_rate_limited(), Some(2_000));
        for _ in 0..20 {
            tokio::time::advance(Duration::from_millis(1_100)).await;
            l.on_rate_limited();
        }
        let st = l.stats();
        assert_eq!(
            (st.initial_rate_milli, st.rate_milli),
            (8_000, MIN_RATE_MILLI)
        );
        assert_eq!(seen.lock().unwrap().first(), Some(&(8_000, 4_000)));
        // after halving to 4/s a token takes 250 ms (bucket was emptied)
        let l = RateLimiter::new(8, 1);
        l.on_rate_limited();
        let t0 = Instant::now();
        l.acquire(1).await.unwrap();
        let el = Instant::now() - t0;
        assert!(el >= Duration::from_millis(250) && el < Duration::from_millis(300));
    }

    #[tokio::test(start_paused = true)]
    async fn rate_recovers_additively_after_quiet_time_and_reports_restore() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s2 = seen.clone();
        let l = RateLimiter::new(80, 80).with_halving_hook(Arc::new(move |a, b| {
            if let Ok(mut v) = s2.lock() {
                v.push((a, b));
            }
        }));
        for _ in 0..4 {
            l.on_rate_limited();
            tokio::time::advance(Duration::from_millis(1_100)).await;
        }
        // 80 -> 5 units/s; a 429 inside the quiet period restarts it
        assert_eq!(l.stats().rate_milli, 5_000);
        tokio::time::advance(RECOVER_STEP - Duration::from_millis(1_200)).await;
        l.on_rate_limited(); // debounced? no: >1 s since the last halving
        assert_eq!(l.stats().rate_milli, 2_500);
        tokio::time::advance(RECOVER_STEP / 2).await;
        l.acquire(1).await.unwrap();
        assert_eq!(l.stats().rate_milli, 2_500, "quiet period not over yet");
        // three steps of 80/8 = 10 units/s each, capped at 80
        tokio::time::advance(RECOVER_STEP * 3).await;
        l.acquire(1).await.unwrap();
        assert_eq!(l.stats().rate_milli, 32_500);
        tokio::time::advance(RECOVER_STEP * 100).await;
        l.acquire(1).await.unwrap();
        let st = l.stats();
        assert_eq!(st.rate_milli, 80_000);
        assert_eq!(st.recoveries, 8);
        assert_eq!(seen.lock().unwrap().last(), Some(&(2_500, 80_000)));
    }

    #[tokio::test(start_paused = true)]
    async fn floor_rate_wait_is_rechecked_after_each_recovery_step() {
        // At the 0.1/s floor a 1-unit token would take 10 s; a 100-unit
        // bucket refill at the floor would take ~17 min. Recovery shortens it.
        let l = RateLimiter::new(100, 100);
        l.acquire(100).await.unwrap();
        for _ in 0..12 {
            l.on_rate_limited();
            tokio::time::advance(Duration::from_millis(1_100)).await;
        }
        assert_eq!(l.stats().rate_milli, MIN_RATE_MILLI);
        let t0 = Instant::now();
        l.acquire(100).await.unwrap();
        let el = Instant::now() - t0;
        assert!(el < Duration::from_secs(60), "{el:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn retune_changes_rate_bucket_and_recovery_target() {
        let l = RateLimiter::new(250, 250);
        l.on_rate_limited();
        l.retune(5_000, 5_000);
        let st = l.stats();
        assert_eq!(
            (st.initial_rate_milli, st.rate_milli),
            (5_000_000, 5_000_000)
        );
        // the bucket starts full at the new size: 5,000 units at once
        let t0 = Instant::now();
        l.acquire(5_000).await.unwrap();
        assert!(Instant::now() - t0 < Duration::from_millis(1));
        // then 5,000 units/s: 500 more take 100 ms
        l.acquire(500).await.unwrap();
        let el = Instant::now() - t0;
        assert!(
            el >= Duration::from_millis(100) && el < Duration::from_millis(120),
            "{el:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn waiter_queue_is_bounded() {
        let l = Arc::new(RateLimiter::new(1, 1).with_max_waiters(2));
        l.acquire(1).await.unwrap();
        let (a, b) = (l.clone(), l.clone());
        let h1 = tokio::spawn(async move { a.acquire(1).await });
        let h2 = tokio::spawn(async move { b.acquire(1).await });
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;
        let r = l.acquire(1).await;
        assert_eq!(r, Err(RateLimiterSaturated { max_waiters: 2 }));
        h1.await.unwrap().unwrap();
        h2.await.unwrap().unwrap();
    }
}
