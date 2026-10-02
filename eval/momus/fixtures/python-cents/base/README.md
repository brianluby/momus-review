# Price import
Upstream validation limits price strings to nonnegative decimal amounts at most 9999999.99, with at most two fractional digits. parse_cents must preserve their exact cent value; 0.29 is 29 cents.
