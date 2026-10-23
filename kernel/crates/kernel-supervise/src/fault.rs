//! `INIT_FAULT` (testing.md §3.8.1): `<point>[@<n>]:<action>[,…]`, action = `abort` |
//! `exit<code>` | `hang` | `enospc` | `sleep<ms>`. Honoured only with the Cargo feature
//! `fault-injection` (never in release builds); otherwise every point is a no-op.
//!
//! Points placed by this crate: kernel/supervise.md §4.10.

use std::io;
use std::path::Path;

#[cfg(feature = "fault-injection")]
mod imp {
    use std::collections::BTreeMap;
    use std::sync::{Mutex, OnceLock};

    #[derive(Clone, Debug)]
    pub enum Action {
        Abort,
        Exit(i32),
        Hang,
        Enospc,
        Sleep(u64),
    }

    pub struct Spec {
        pub at: u64,
        pub action: Action,
        pub seen: u64,
    }

    static SPECS: OnceLock<Mutex<BTreeMap<String, Spec>>> = OnceLock::new();

    pub fn parse(s: &str) -> BTreeMap<String, Spec> {
        let mut m = BTreeMap::new();
        for item in s.split(',').map(str::trim).filter(|x| !x.is_empty()) {
            let Some((point, action)) = item.rsplit_once(':') else { continue };
            let (point, at) = match point.split_once('@') {
                Some((p, n)) => (p, n.parse().unwrap_or(1)),
                None => (point, 1),
            };
            let action = match action {
                "abort" => Action::Abort,
                "hang" => Action::Hang,
                "enospc" => Action::Enospc,
                a if a.starts_with("exit") => Action::Exit(a[4..].parse().unwrap_or(1)),
                a if a.starts_with("sleep") => Action::Sleep(a[5..].parse().unwrap_or(0)),
                _ => continue,
            };
            m.insert(point.to_string(), Spec { at, action, seen: 0 });
        }
        m
    }

    pub fn specs() -> &'static Mutex<BTreeMap<String, Spec>> {
        SPECS.get_or_init(|| Mutex::new(parse(&std::env::var("INIT_FAULT").unwrap_or_default())))
    }

    /// Replace the specs (in-process tests).
    pub fn set(s: &str) {
        *specs().lock().unwrap() = parse(s);
    }
}

/// Reach fault point `point`. `Err(StorageFull)` for `enospc`; the other actions act here.
#[allow(unused_variables)]
pub fn hit(point: &str, path: &Path) -> io::Result<()> {
    #[cfg(feature = "fault-injection")]
    {
        let action = {
            let mut g = imp::specs().lock().unwrap();
            match g.get_mut(point) {
                Some(s) => {
                    s.seen += 1;
                    if s.seen == s.at { Some(s.action.clone()) } else { None }
                }
                None => None,
            }
        };
        if let Some(a) = action {
            crate::klog::log("fault", &format!("INIT_FAULT {point} fired: {a:?}"));
            match a {
                imp::Action::Abort => std::process::abort(),
                imp::Action::Exit(c) => std::process::exit(c),
                imp::Action::Hang => loop {
                    std::thread::sleep(std::time::Duration::from_secs(3600));
                },
                imp::Action::Enospc => return Err(io::Error::new(io::ErrorKind::StorageFull, format!("INIT_FAULT enospc at {point}"))),
                imp::Action::Sleep(ms) => std::thread::sleep(std::time::Duration::from_millis(ms)),
            }
        }
    }
    Ok(())
}

/// A point with no file context.
pub fn point(p: &str) {
    let _ = hit(p, Path::new(""));
}

/// Replace the active fault specs (in-process tests; no-op without the feature).
#[allow(unused_variables)]
pub fn set_for_test(spec: &str) {
    #[cfg(feature = "fault-injection")]
    imp::set(spec);
}
