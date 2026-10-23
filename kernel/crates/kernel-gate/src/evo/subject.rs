//! The subject line (evolution.md §3.8 ABNF; §3.9 step 4).
//!
//! ```abnf
//! subject   = type [ "(" scope ")" ] ": " summary
//! scope     = 1*64( ALPHA / DIGIT / "-" / "_" / "/" / "." )
//! summary   = 1*( %x20-7E / UTF8-non-ascii )     ; no trailing whitespace
//! ```
//! The subject is at most 72 bytes and a single line.

use super::{ChangeType, ValidationError, codes};

/// Maximum subject length in bytes (§3.8).
pub const MAX_SUBJECT_BYTES: usize = 72;

/// A parsed subject.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subject {
    pub change_type: ChangeType,
    pub scope: Option<String>,
    pub summary: String,
}

fn is_scope(s: &str) -> bool {
    (1..=64).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'/' | b'.'))
}

fn is_summary_char(c: char) -> bool {
    (' '..='~').contains(&c) || !c.is_ascii()
}

/// Parses one subject line (without its line terminator). Returns the subject when the grammar holds
/// (even if it is too long, so later steps still know the type) and every problem found.
pub fn parse_subject(line: &str) -> (Option<Subject>, Vec<ValidationError>) {
    let mut errors = Vec::new();
    if line.len() > MAX_SUBJECT_BYTES {
        errors.push(ValidationError::new(
            codes::SUBJECT_TOO_LONG,
            format!(
                "subject is {} bytes; the limit is {MAX_SUBJECT_BYTES}",
                line.len()
            ),
        ));
    }
    if line.ends_with(char::is_whitespace) {
        errors.push(ValidationError::new(
            codes::SUBJECT,
            "subject has trailing whitespace",
        ));
        return (None, errors);
    }
    let Some((head, summary)) = line.split_once(": ") else {
        errors.push(ValidationError::new(
            codes::SUBJECT,
            format!("expected '<type>[(<scope>)]: <summary>', got '{line}'"),
        ));
        return (None, errors);
    };
    let (ty, scope) = match head.split_once('(') {
        Some((ty, rest)) => match rest.strip_suffix(')') {
            Some(scope) if is_scope(scope) => (ty, Some(scope.to_string())),
            _ => {
                errors.push(ValidationError::new(
                    codes::SUBJECT,
                    format!("scope in '{head}' must be 1-64 of [A-Za-z0-9-_/.] inside parentheses"),
                ));
                return (None, errors);
            }
        },
        None => (head, None),
    };
    let Some(change_type) = ChangeType::parse(ty) else {
        errors.push(ValidationError::new(
            codes::SUBJECT,
            format!(
                "unknown type '{ty}'; expected one of {}",
                ChangeType::ALL.map(ChangeType::as_str).join(", ")
            ),
        ));
        return (None, errors);
    };
    if summary.is_empty() || !summary.chars().all(is_summary_char) {
        errors.push(ValidationError::new(
            codes::SUBJECT,
            "summary must be non-empty printable text (no control characters)",
        ));
        return (None, errors);
    }
    (
        Some(Subject {
            change_type,
            scope,
            summary: summary.to_string(),
        }),
        errors,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codes_of(line: &str) -> Vec<String> {
        parse_subject(line).1.into_iter().map(|e| e.code).collect()
    }

    #[test]
    fn valid_subjects() {
        let (s, e) = parse_subject("feat(ext): add pty component for interactive programs");
        assert!(e.is_empty());
        let s = s.unwrap();
        assert_eq!(s.change_type, ChangeType::Feat);
        assert_eq!(s.scope.as_deref(), Some("ext"));
        assert_eq!(s.summary, "add pty component for interactive programs");
        let (s, e) = parse_subject("revert: undo ünïcode change");
        assert!(e.is_empty());
        assert_eq!(s.unwrap().scope, None);
        for t in ChangeType::ALL {
            assert!(
                parse_subject(&format!("{}(a/b_c.d-e): x", t.as_str()))
                    .1
                    .is_empty()
            );
        }
    }

    #[test]
    fn invalid_subjects() {
        assert_eq!(codes_of("feature: x"), vec![codes::SUBJECT]);
        assert_eq!(codes_of("Feat: x"), vec![codes::SUBJECT]);
        assert_eq!(codes_of("feat:x"), vec![codes::SUBJECT]);
        assert_eq!(codes_of("feat(): x"), vec![codes::SUBJECT]);
        assert_eq!(codes_of("feat(a b): x"), vec![codes::SUBJECT]);
        assert_eq!(codes_of("feat: x "), vec![codes::SUBJECT]);
        assert_eq!(codes_of("feat: "), vec![codes::SUBJECT]);
        assert_eq!(codes_of("feat: a\tb"), vec![codes::SUBJECT]);
        assert_eq!(
            codes_of(&format!("feat({}): x", "s".repeat(65))),
            vec![codes::SUBJECT_TOO_LONG, codes::SUBJECT]
        );
    }

    #[test]
    fn length_limit_is_72_bytes() {
        let ok = format!("fix: {}", "x".repeat(MAX_SUBJECT_BYTES - 5));
        assert_eq!(ok.len(), 72);
        assert!(codes_of(&ok).is_empty());
        let long = format!("fix: {}", "x".repeat(MAX_SUBJECT_BYTES - 4));
        let (s, e) = parse_subject(&long);
        assert_eq!(
            e.iter().map(|e| e.code.as_str()).collect::<Vec<_>>(),
            vec![codes::SUBJECT_TOO_LONG]
        );
        assert_eq!(
            s.unwrap().change_type,
            ChangeType::Fix,
            "type survives a length error"
        );
        // Bytes, not characters: 34 two-byte characters plus "fix: " is 73 bytes.
        assert_eq!(
            codes_of(&format!("fix: {}", "é".repeat(34))),
            vec![codes::SUBJECT_TOO_LONG]
        );
    }
}
