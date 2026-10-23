// Reference solution of e-rs-isbn-verifier (rendered by gen/run with the seeded constants).
const SEPARATOR: char = @SEPARATOR@;
const STRICT_PLACEMENT: bool = @STRICT@;
const CASE_SENSITIVE: bool = @CASE_SENSITIVE@;
const LENGTH: usize = @LENGTH@;
const MODULUS: u64 = @MODULUS@;
const WEIGHTS: [u64; @LENGTH@] = [@WEIGHTS@];
const CHECK_LETTERS: &[(char, u64)] = &[@LETTERS@];

fn letter_value(c: char) -> Option<u64> {
    let c = if CASE_SENSITIVE { c } else { c.to_ascii_uppercase() };
    CHECK_LETTERS.iter().find(|(l, _)| *l == c).map(|(_, v)| *v)
}

/// Whether `code` is a valid code under the rules of the task.
pub fn is_valid_code(code: &str) -> bool {
    let chars: Vec<char> = code.chars().collect();
    let mut symbols: Vec<char> = Vec::new();
    for (i, &c) in chars.iter().enumerate() {
        if c == SEPARATOR {
            if STRICT_PLACEMENT {
                let first_or_last = i == 0 || i + 1 == chars.len();
                if first_or_last || chars[i - 1] == SEPARATOR || chars[i + 1] == SEPARATOR {
                    return false;
                }
            }
        } else {
            symbols.push(c);
        }
    }
    if symbols.len() != LENGTH {
        return false;
    }
    let mut total: u64 = 0;
    for (i, &c) in symbols.iter().enumerate() {
        let value = if c.is_ascii_digit() {
            c as u64 - '0' as u64
        } else if i + 1 == LENGTH {
            match letter_value(c) {
                Some(v) => v,
                None => return false,
            }
        } else {
            return false;
        };
        total += WEIGHTS[i] * value;
    }
    total % MODULUS == 0
}
