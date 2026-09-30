use std::sync::atomic::{AtomicU64, Ordering};
#[derive(Debug)]
pub struct BudgetExhausted;
impl std::fmt::Display for BudgetExhausted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("budget_exhausted: request queued for a cached rerun")
    }
}
impl std::error::Error for BudgetExhausted {}
pub struct CallBudget {
    limit: Option<u64>,
    reserved: AtomicU64,
    deferred: AtomicU64,
}
impl CallBudget {
    pub fn new(limit: Option<u64>) -> Self {
        Self {
            limit,
            reserved: AtomicU64::new(0),
            deferred: AtomicU64::new(0),
        }
    }
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
