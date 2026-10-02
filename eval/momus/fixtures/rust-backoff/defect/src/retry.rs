pub fn delay_ms(attempt: u32) -> u64 {
    (100 * (1u64 << attempt)).min(10000)
}
#[cfg(test)] mod tests {
    #[test] fn initial_delay() { assert_eq!(super::delay_ms(0), 100); }
}
