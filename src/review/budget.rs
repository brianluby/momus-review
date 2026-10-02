//! Shared reservation accounting for outbound model attempts.
//!
//! The client checks its cache before reserving an HTTP attempt. Reservations
//! count attempts, including retries and failed requests, rather than successful
//! judgments. This module accounts for work; it does not queue or resume it.

use std::sync::atomic::{AtomicU64, Ordering};
/// The configured attempt ceiling prevented a reservation.
///
/// The display text mentions a cached rerun, but this error does not itself
/// enqueue work or guarantee that a later run will complete it. The caller must
/// record deferred evidence and choose how to resume.
#[derive(Debug)]
pub struct BudgetExhausted;
impl std::fmt::Display for BudgetExhausted {
    /// Explain why uncached work was deferred without implying that it completed.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("budget_exhausted: request queued for a cached rerun")
    }
}
impl std::error::Error for BudgetExhausted {}
/// An atomic, shared counter with an optional lifetime attempt ceiling.
///
/// Successful reservations are never refunded. Share a single instance among
/// workers when the limit should apply to the whole run. A finite ceiling cannot
/// be exceeded by concurrent reservations.
///
/// ```
/// use momus_review::review::budget::CallBudget;
///
/// let budget = CallBudget::new(Some(1));
/// budget.reserve()?; // Reserve before the HTTP attempt, even if it later fails.
/// assert!(budget.reserve().is_err());
/// let summary = budget.summary();
/// assert_eq!(summary.limit, Some(1));
/// assert_eq!(summary.reserved, 1);
/// assert_eq!(summary.deferred, 1);
/// # Ok::<(), momus_review::review::budget::BudgetExhausted>(())
/// ```
pub struct CallBudget {
    limit: Option<u64>,
    reserved: AtomicU64,
    deferred: AtomicU64,
}
impl CallBudget {
    /// Create a counter: `None` imposes no ceiling; `Some(0)` rejects every
    /// reservation. Cached results need no reservation at the client layer.
    ///
    /// ```
    /// use momus_review::review::budget::CallBudget;
    ///
    /// let no_attempts = CallBudget::new(Some(0));
    /// assert!(no_attempts.reserve().is_err());
    /// assert_eq!(no_attempts.summary().reserved, 0);
    /// assert_eq!(no_attempts.summary().deferred, 1);
    /// assert!(CallBudget::new(None).reserve().is_ok());
    /// ```
    pub fn new(limit: Option<u64>) -> Self {
        Self {
            limit,
            reserved: AtomicU64::new(0),
            deferred: AtomicU64::new(0),
        }
    }
    /// Atomically reserve one attempt before performing its work.
    ///
    /// Each successful call increases `reserved` once. Each rejected call
    /// increases `deferred` once, so repeated attempts to reserve the same logical
    /// work count repeatedly. Reservations cannot be canceled or refunded.
    ///
    /// # Errors
    ///
    /// Returns [`BudgetExhausted`] when the finite ceiling has been reached.
    // Keep the API available at the declared Rust 1.88 MSRV; its renamed
    // replacement is newer. This allowance is limited to this compatibility call.
    #[allow(deprecated)]
    pub fn reserve(&self) -> Result<(), BudgetExhausted> {
        if self
            .reserved
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                if self.limit.is_some_and(|limit| n >= limit) {
                    None
                } else {
                    Some(n + 1)
                }
            })
            .is_err()
        {
            self.deferred.fetch_add(1, Ordering::Relaxed);
            return Err(BudgetExhausted);
        }
        Ok(())
    }
    /// Read the configured limit and reserved/deferred attempt counters.
    ///
    /// The counters use independent relaxed atomic loads. During concurrent
    /// work this is an approximate observation, not one atomic snapshot of both
    /// counters; read it after workers finish for final accounting.
    pub fn summary(&self) -> crate::domain::report::BudgetSummary {
        crate::domain::report::BudgetSummary {
            limit: self.limit,
            reserved: self.reserved.load(Ordering::Relaxed),
            deferred: self.deferred.load(Ordering::Relaxed),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn concurrent_reservations_never_exceed_the_cap() {
        let budget = std::sync::Arc::new(CallBudget::new(Some(7)));
        let threads: Vec<_> = (0..32)
            .map(|_| {
                let b = budget.clone();
                std::thread::spawn(move || b.reserve().is_ok())
            })
            .collect();
        assert_eq!(
            threads
                .into_iter()
                .filter_map(|t| t.join().ok())
                .filter(|b| *b)
                .count(),
            7
        );
        assert_eq!(budget.summary().reserved, 7);
        assert_eq!(budget.summary().deferred, 25);
    }
}
