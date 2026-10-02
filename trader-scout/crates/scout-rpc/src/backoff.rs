//! Integer-only retry/backoff. Per workspace lint policy
//! (`float_arithmetic = "deny"`), no delay computation here uses
//! floats — everything is whole milliseconds (`u64`), doubled by
//! bit-shift/checked-multiply, jittered by an injectable integer
//! source so backoff behavior is deterministic and testable.

use std::time::Duration;

/// A source of jitter for backoff delays. Injectable (not
/// `rand::thread_rng()` called directly) so tests can supply a fixed
/// sequence and get deterministic, reproducible delay values — the
/// workspace determinism policy applies to test infrastructure too,
/// not just ledger arithmetic.
pub trait JitterSource: Send + Sync {
    /// Returns a jitter value in `0..bound` (exclusive). Implementors
    /// must not panic; `bound == 0` must return `0`.
    fn next_jitter_ms(&self, bound_ms: u64) -> u64;
}

/// Jitter from a simple xorshift-style counter — no external RNG
/// dependency, fully deterministic given a seed, safe for tests and
/// good enough for backoff (this is not cryptographic).
#[derive(Debug, Clone)]
pub struct CounterJitter {
    state: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

impl CounterJitter {
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self {
            state: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(seed | 1)),
        }
    }
}

impl Default for CounterJitter {
    fn default() -> Self {
        // A fixed, non-zero default seed: reproducible unless the
        // caller explicitly wants a different one via `new`.
        Self::new(0x9E37_79B9_7F4A_7C15)
    }
}

impl JitterSource for CounterJitter {
    fn next_jitter_ms(&self, bound_ms: u64) -> u64 {
        if bound_ms == 0 {
            return 0;
        }
        // xorshift64
        let mut x = self.state.load(std::sync::atomic::Ordering::Relaxed);
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state.store(x, std::sync::atomic::Ordering::Relaxed);
        x % bound_ms
    }
}

/// A jitter source that always returns zero — for tests that want
/// exact, unjittered backoff values.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoJitter;

impl JitterSource for NoJitter {
    fn next_jitter_ms(&self, _bound_ms: u64) -> u64 {
        0
    }
}

/// Boxed future returned by [`Sleeper::sleep`].
pub type SleepFuture<'a> = std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>>;

/// Injectable wait between retry attempts, so tests can record the
/// requested delays instead of really sleeping.
pub trait Sleeper: Send + Sync {
    fn sleep(&self, duration: Duration) -> SleepFuture<'_>;
}

/// Production sleeper: `tokio::time::sleep`.
#[derive(Debug, Clone, Copy, Default)]
pub struct TokioSleeper;

impl Sleeper for TokioSleeper {
    fn sleep(&self, duration: Duration) -> SleepFuture<'_> {
        Box::pin(tokio::time::sleep(duration))
    }
}

/// Retry/backoff policy. All delays are whole milliseconds; doubling
/// uses `saturating_mul` (never silently wraps or panics on overflow,
/// per AGENTS.md invariant #7's checked-arithmetic discipline extended
/// to non-monetary integer math here for the same reason: no silent
/// wrong answers).
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub base_delay_ms: u64,
    pub max_delay_ms: u64,
}

impl RetryPolicy {
    /// Matches `config/scout.example.toml`'s `[scan]` defaults
    /// (`max_attempts = 5`) rather than inventing a separate number —
    /// this crate reads that config field once wired, not a value that
    /// silently diverges from it.
    #[must_use]
    pub fn from_config(max_attempts: u32, request_timeout_ms: u64) -> Self {
        Self {
            max_attempts,
            base_delay_ms: 250,
            // Never wait longer than one request timeout for a single
            // backoff step — a delay that already exceeds the timeout
            // budget is not useful to wait out.
            max_delay_ms: request_timeout_ms,
        }
    }

    /// Delay before retry attempt `attempt` (1-indexed: the delay
    /// before the *second* try is `delay_for_attempt(1)`). Doubles per
    /// attempt via bit-shift, capped at `max_delay_ms`, then jittered
    /// by up to the base delay's worth of randomness so many
    /// concurrent callers don't retry in lockstep.
    #[must_use]
    pub fn delay_for_attempt(&self, attempt: u32, jitter: &dyn JitterSource) -> Duration {
        let shift = attempt.min(20); // avoid absurd shift amounts
        let doubled = self.base_delay_ms.saturating_mul(1u64 << shift);
        let capped = doubled.min(self.max_delay_ms);
        let jitter_bound = self.base_delay_ms.min(capped.max(1));
        let jittered = capped.saturating_add(jitter.next_jitter_ms(jitter_bound));
        Duration::from_millis(jittered.min(self.max_delay_ms.saturating_add(self.base_delay_ms)))
    }

    /// Parse an HTTP `Retry-After` header value. Only the integer-
    /// seconds form is supported (`Retry-After: 120`); the HTTP-date
    /// form (`Retry-After: Wed, 21 Oct 2026 07:28:00 GMT`) is
    /// deliberately not parsed here — falling back to the policy's own
    /// backoff is safer than adding a date-parsing dependency for a
    /// case none of our measured providers (Helius, Blockscout, Codex)
    /// were observed to use. Never panics on malformed input.
    #[must_use]
    pub fn parse_retry_after(value: &str) -> Option<Duration> {
        value.trim().parse::<u64>().ok().map(Duration::from_secs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delay_doubles_per_attempt_without_jitter() {
        let policy = RetryPolicy {
            max_attempts: 5,
            base_delay_ms: 100,
            max_delay_ms: 10_000,
        };
        assert_eq!(
            policy.delay_for_attempt(0, &NoJitter),
            Duration::from_millis(100)
        );
        assert_eq!(
            policy.delay_for_attempt(1, &NoJitter),
            Duration::from_millis(200)
        );
        assert_eq!(
            policy.delay_for_attempt(2, &NoJitter),
            Duration::from_millis(400)
        );
        assert_eq!(
            policy.delay_for_attempt(3, &NoJitter),
            Duration::from_millis(800)
        );
    }

    #[test]
    fn delay_is_capped_at_max_delay_ms() {
        let policy = RetryPolicy {
            max_attempts: 10,
            base_delay_ms: 100,
            max_delay_ms: 500,
        };
        // attempt 10 would be 100 * 2^10 = 102400ms without a cap.
        let delay = policy.delay_for_attempt(10, &NoJitter);
        assert!(delay <= Duration::from_millis(500 + 100));
    }

    #[test]
    fn delay_never_panics_on_large_attempt_numbers() {
        let policy = RetryPolicy {
            max_attempts: 5,
            base_delay_ms: 250,
            max_delay_ms: 15_000,
        };
        // Must not panic (shift overflow, multiply overflow) even for
        // attempt numbers far beyond max_attempts.
        let _ = policy.delay_for_attempt(u32::MAX, &NoJitter);
        let _ = policy.delay_for_attempt(1000, &NoJitter);
    }

    #[test]
    fn jitter_source_is_injectable_and_deterministic() {
        let jitter_a = CounterJitter::new(42);
        let jitter_b = CounterJitter::new(42);
        let policy = RetryPolicy {
            max_attempts: 5,
            base_delay_ms: 100,
            max_delay_ms: 10_000,
        };
        // Same seed -> same sequence of delays.
        let seq_a: Vec<_> = (0..5)
            .map(|i| policy.delay_for_attempt(i, &jitter_a))
            .collect();
        let seq_b: Vec<_> = (0..5)
            .map(|i| policy.delay_for_attempt(i, &jitter_b))
            .collect();
        assert_eq!(seq_a, seq_b);
    }

    #[test]
    fn parse_retry_after_handles_integer_seconds() {
        assert_eq!(
            RetryPolicy::parse_retry_after("120"),
            Some(Duration::from_secs(120))
        );
        assert_eq!(
            RetryPolicy::parse_retry_after("0"),
            Some(Duration::from_secs(0))
        );
    }

    #[test]
    fn parse_retry_after_never_panics_on_garbage() {
        assert_eq!(RetryPolicy::parse_retry_after(""), None);
        assert_eq!(RetryPolicy::parse_retry_after("not-a-number"), None);
        // HTTP-date form: deliberately unsupported, must return None,
        // not panic.
        assert_eq!(
            RetryPolicy::parse_retry_after("Wed, 21 Oct 2026 07:28:00 GMT"),
            None
        );
    }

    #[test]
    fn parse_retry_after_trims_whitespace() {
        assert_eq!(
            RetryPolicy::parse_retry_after("  30  "),
            Some(Duration::from_secs(30))
        );
    }
}
