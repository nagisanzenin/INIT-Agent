// Reference solution of e-rs-luhn (rendered by gen/run with the seeded constants).
const SEPARATORS: &[char] = &[@SEPARATORS@];
const STRICT: bool = @STRICT@;
const MIN_DIGITS: usize = @MIN_DIGITS@;
const PERIOD: usize = @PERIOD@;
const MULTIPLIER: u32 = @MULTIPLIER@;
const MODULUS: u32 = @MODULUS@;

/// Whether `code` passes the check-digit rules of the task.
pub fn is_valid(code: &str) -> bool {
    let chars: Vec<char> = code.chars().collect();
    let mut digits: Vec<u32> = Vec::new();
    for (i, &c) in chars.iter().enumerate() {
        if c.is_ascii_digit() {
            digits.push(c as u32 - '0' as u32);
        } else if SEPARATORS.contains(&c) {
            if STRICT {
                let left = i > 0 && chars[i - 1].is_ascii_digit();
                let right = i + 1 < chars.len() && chars[i + 1].is_ascii_digit();
                if !(left && right) {
                    return false;
                }
            }
        } else {
            return false;
        }
    }
    if digits.len() < MIN_DIGITS {
        return false;
    }
    let mut total: u64 = 0;
    for (idx, &d) in digits.iter().rev().enumerate() {
        let position = idx + 1;
        let value = if position % PERIOD == 0 {
            let p = d * MULTIPLIER;
            if p > 9 { p / 10 + p % 10 } else { p }
        } else {
            d
        };
        total += value as u64;
    }
    total % MODULUS as u64 == 0
}
