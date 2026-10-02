mod invoice;
fn main() {
    assert_eq!(invoice::discounted_total(2000, 2500), 1500);
    assert_eq!(invoice::discounted_total(2001, 2500), 1501);
    assert_eq!(invoice::discounted_total(u64::MAX, 10000), 0);
    assert_eq!(invoice::discounted_total(u64::MAX, 0), u64::MAX);
}
