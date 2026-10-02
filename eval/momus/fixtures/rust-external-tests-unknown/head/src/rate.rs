pub fn percent(value: u8) -> f64 { f64::from(value) / 100.0 }
pub fn ratio(numerator: u8, denominator: u8) -> Option<f64> {
    if denominator == 0 { None } else { Some(f64::from(numerator) / f64::from(denominator)) }
}
