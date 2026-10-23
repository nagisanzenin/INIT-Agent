//! Small std-only validators shared by the protocol types (no regex crate).

/// `^[0-9a-f]{40}$`: a full lowercase commit SHA.
pub fn is_sha40(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// `^[0-9a-f]{64}$`: a lowercase SHA-256.
pub fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// A request id: 1–64 bytes of `[A-Za-z0-9._:-]` (kernel/initctl.md §3.2).
pub fn is_request_id(s: &str) -> bool {
    (1..=64).contains(&s.len())
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'-'))
}

/// `^[A-Za-z0-9_-]{1,64}$`: a crate name for `vendor`.
pub fn is_crate_name(s: &str) -> bool {
    (1..=64).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

/// An exact semver: `^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$`.
pub fn is_exact_semver(s: &str) -> bool {
    let (core_pre, build) = match s.split_once('+') {
        Some((a, b)) => (a, Some(b)),
        None => (s, None),
    };
    let (core, pre) = match core_pre.split_once('-') {
        Some((a, b)) => (a, Some(b)),
        None => (core_pre, None),
    };
    let ident_ok = |x: &str| !x.is_empty() && x.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-'));
    let parts: Vec<&str> = core.split('.').collect();
    parts.len() == 3
        && parts.iter().all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
        && pre.is_none_or(ident_ok)
        && build.is_none_or(ident_ok)
}

/// `^kevo/[a-z0-9][a-z0-9._-]{0,62}$` (kernel/initctl.md `kernel promote`).
pub fn is_kevo_ref(s: &str) -> bool {
    match s.strip_prefix("kevo/") {
        Some(id) => {
            let b = id.as_bytes();
            !b.is_empty()
                && b.len() <= 63
                && matches!(b[0], b'a'..=b'z' | b'0'..=b'9')
                && b.iter().all(|c| matches!(c, b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-'))
        }
        None => false,
    }
}

/// `evo/<id>`: a non-empty branch under `evo/`, at most 255 bytes, no whitespace or control
/// characters. The kernel still runs `git check-ref-format` (kernel/initctl.md §7).
pub fn is_evo_ref(s: &str) -> bool {
    s.len() <= 255
        && s.strip_prefix("evo/").is_some_and(|id| !id.is_empty())
        && !s.bytes().any(|b| b.is_ascii_whitespace() || b.is_ascii_control())
}

/// A job id: `j-` followed by at least 6 digits (`j-000043`).
pub fn is_job_id(s: &str) -> bool {
    s.strip_prefix("j-").is_some_and(|d| d.len() >= 6 && d.bytes().all(|b| b.is_ascii_digit()))
}

/// `<provider>/<id>`: two non-empty parts without whitespace.
pub fn is_model_ref(s: &str) -> bool {
    match s.split_once('/') {
        Some((p, id)) => {
            !p.is_empty() && !id.is_empty() && !s.bytes().any(|b| b.is_ascii_whitespace() || b.is_ascii_control())
        }
        None => false,
    }
}

/// Cut `s` to at most `max` bytes on a char boundary and make it one line
/// (error messages: "one line of human-readable text, at most 500 bytes").
pub fn one_line(s: &str, max: usize) -> String {
    let flat: String = s.chars().map(|c| if c == '\n' || c == '\r' { ' ' } else { c }).collect();
    if flat.len() <= max {
        return flat;
    }
    let mut end = max;
    while !flat.is_char_boundary(end) {
        end -= 1;
    }
    flat[..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validators() {
        assert!(is_sha40("4b1e0c2d9a7f3e5d1c0b9a8f7e6d5c4b3a291807"));
        assert!(!is_sha40("4B1E0C2D9A7F3E5D1C0B9A8F7E6D5C4B3A291807"));
        assert!(!is_sha40("4b1e"));
        assert!(is_request_id("48121-1") && is_request_id("k-42") && !is_request_id("") && !is_request_id("a b"));
        assert!(!is_request_id(&"x".repeat(65)));
        assert!(is_crate_name("vt100") && !is_crate_name("a/b"));
        for ok in ["0.16.2", "1.0.0-rc.1", "1.0.0+build.5", "1.0.0-alpha+001"] {
            assert!(is_exact_semver(ok), "{ok}");
        }
        for bad in ["1.0", "^1.0.0", "1.0.0-", "1.0.0+", "a.b.c", "1.0.0.0"] {
            assert!(!is_exact_semver(bad), "{bad}");
        }
        assert!(is_kevo_ref("kevo/gate-speedup") && !is_kevo_ref("kevo/Upper") && !is_kevo_ref("evo/x"));
        assert!(is_evo_ref("evo/pty-tool") && !is_evo_ref("evo/") && !is_evo_ref("main") && !is_evo_ref("evo/a b"));
        assert!(is_job_id("j-000043") && !is_job_id("j-43") && !is_job_id("x-000043"));
        assert!(is_model_ref("deepseek/deepseek-flash") && !is_model_ref("deepseek-flash"));
        assert_eq!(one_line("a\nb", 10), "a b");
        assert_eq!(one_line("ééé", 5), "éé");
    }
}
