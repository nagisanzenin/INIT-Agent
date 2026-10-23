// Reference solution of e-rs-armstrong-numbers (rendered by gen/run with the seeded constants).
const BASE: u64 = @BASE@;
const OFFSET: u32 = @OFFSET@;

/// Whether `num` is narcissistic under the rules of the task.
pub fn is_armstrong_number(num: u64) -> bool {
    // base-BASE digits, least significant first; 0 has one digit
    let mut digits: Vec<u64> = Vec::new();
    let mut rest = num;
    while rest > 0 {
        digits.push(rest % BASE);
        rest /= BASE;
    }
    if digits.is_empty() {
        digits.push(0);
    }
    let exponent = digits.len() as u32 + OFFSET;
    let mut sum: u64 = 0;
    for &d in &digits {
        // a power or a sum above u64::MAX can never equal num
        let term = match d.checked_pow(exponent) {
            Some(t) => t,
            None => return false,
        };
        sum = match sum.checked_add(term) {
            Some(s) => s,
            None => return false,
        };
    }
    sum == num
}
