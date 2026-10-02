pub fn discounted_total(cents: u64, basis_points: u16) -> u64 {
    assert!(basis_points <= 10000);
    cents - cents * (u64::from(basis_points) / 10000)
}
#[cfg(test)] mod tests {
    #[test] fn no_discount() { assert_eq!(super::discounted_total(2000, 0), 2000); }
}
