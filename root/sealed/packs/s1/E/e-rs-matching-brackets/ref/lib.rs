// Reference solution of e-rs-matching-brackets (rendered by gen/run with the seeded constants).
const PAIRS: &[(char, char)] = &[@PAIRS@];
const QUOTE: Option<char> = @QUOTE@;
const MAX_DEPTH: usize = @MAX_DEPTH@;

/// Whether the active brackets in `string` are balanced under the task's rules.
pub fn brackets_are_balanced(string: &str) -> bool {
    let mut stack: Vec<char> = Vec::new();
    let mut quoted = false;
    for c in string.chars() {
        if quoted {
            if Some(c) == QUOTE {
                quoted = false;
            }
            continue;
        }
        if Some(c) == QUOTE {
            quoted = true;
        } else if let Some(&(_, close)) = PAIRS.iter().find(|&&(open, _)| open == c) {
            stack.push(close);
            if stack.len() > MAX_DEPTH {
                return false;
            }
        } else if PAIRS.iter().any(|&(_, close)| close == c) {
            if stack.pop() != Some(c) {
                return false;
            }
        }
    }
    !quoted && stack.is_empty()
}
