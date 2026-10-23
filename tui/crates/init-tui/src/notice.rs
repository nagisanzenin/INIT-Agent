//! Notice classes (docs/tui.md §3.10). For styling only: no TUI behaviour except an immediate status
//! poll depends on a notice's text. With the Root's `src` stamp (§9 Q1), the Root classes require
//! `src:"root"`, so a kernel cannot dress its notices up as the Root's.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Promotion,
    LevelUp,
    Rollback,
    Demotion,
    Ceiling,
    Panic,
    SafeMode,
    Halt,
    /// Anything else, styled by its level.
    Plain,
}

/// Classifies a notice. `src` is the Root's stamp when present; `code` the Root's alert code.
pub fn classify(text: &str, src: Option<&str>, code: Option<&str>) -> Class {
    let t = text.trim_start();
    // As built (no `src`), the text is all there is; with `src`, a `[root]` look-alike from the kernel
    // is a plain kernel notice.
    let rootish = match src {
        Some(s) => s == "root",
        None => true,
    };
    if let Some(k) = t.strip_prefix("[kernel] ") {
        if k.starts_with("promoted") || k.starts_with("lkg") || k.starts_with("provisional") {
            return Class::Promotion;
        }
        if k.starts_with("update_failed") {
            return Class::Rollback;
        }
        return Class::Plain;
    }
    if !rootish {
        return Class::Plain;
    }
    if let Some(c) = code {
        match c {
            "level_up" => return Class::LevelUp,
            "demoted" => return Class::Demotion,
            "rollback" | "kernel_crash_loop" => return Class::Rollback,
            "panic" => return Class::Panic,
            "ceiling_hit" => return Class::Ceiling,
            "halted" => return Class::Halt,
            "system_safe" | "safe_mode" => return Class::SafeMode,
            _ => {}
        }
    }
    if t.starts_with("PANIC") {
        return Class::Panic;
    }
    let Some(r) = t.strip_prefix("[root] ") else { return Class::Plain };
    if r.starts_with("level L") {
        return Class::LevelUp;
    }
    if r.starts_with("demoted L") {
        return Class::Demotion;
    }
    if r.contains("falling back to LKG") || r.starts_with("kernel crash loop") {
        return Class::Rollback;
    }
    if r.starts_with("system safe mode") {
        return Class::SafeMode;
    }
    if r.starts_with("Halted{") {
        return Class::Halt;
    }
    if r.starts_with("ceiling reached") {
        return Class::Ceiling;
    }
    Class::Plain
}

/// Is this the Root's greeting at attach (`[root] attached: …`)?
pub fn is_greeting(text: &str, src: Option<&str>) -> bool {
    src.is_none_or(|s| s == "root") && text.starts_with("[root] attached:")
}

/// Is this the Root's notice that the owner queue refused a line?
pub fn is_queue_full(text: &str, src: Option<&str>) -> bool {
    src.is_none_or(|s| s == "root") && text.starts_with("[root] the owner queue is full")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classes() {
        assert_eq!(classify("[kernel] promoted", Some("kernel"), None), Class::Promotion);
        assert_eq!(classify("[kernel] update_failed x", None, None), Class::Rollback);
        assert_eq!(classify("[root] level L0→L1: ok", None, None), Class::LevelUp);
        assert_eq!(classify("[root] demoted L1→L0 (x)", None, None), Class::Demotion);
        assert_eq!(classify("[root] kernel crash loop: rolling back to the LKG tuple", None, None), Class::Rollback);
        assert_eq!(classify("PANIC at 2026-10-20T00:00:00Z", None, None), Class::Panic);
        assert_eq!(classify("[root] system safe mode on", None, None), Class::SafeMode);
        assert_eq!(classify("[root] Halted{lineage}: x", None, None), Class::Halt);
        assert_eq!(classify("anything", None, None), Class::Plain);
        assert_eq!(classify("[root] x", Some("root"), Some("ceiling_hit")), Class::Ceiling);
    }

    #[test]
    fn kernel_cannot_forge_root_classes_once_stamped() {
        assert_eq!(classify("[root] demoted L1→L0 (x)", Some("kernel"), None), Class::Plain);
        assert_eq!(classify("PANIC at now", Some("kernel"), Some("panic")), Class::Plain);
        assert_eq!(classify("[root] demoted L1→L0 (x)", Some("root"), None), Class::Demotion);
        assert!(!is_greeting("[root] attached: slot", Some("kernel")));
        assert!(is_greeting("[root] attached: slot", None));
    }
}
