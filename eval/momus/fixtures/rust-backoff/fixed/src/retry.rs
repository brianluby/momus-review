pub fn delay_ms(attempt: u32) -> u64 {
    100u64.checked_mul(2u64.saturating_pow(attempt)).unwrap_or(10000).min(10000)
}
#[cfg(test)] mod tests {
    #[test] fn initial_delay() { assert_eq!(super::delay_ms(0), 100); }
}
