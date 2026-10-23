//! The `INIT_FAULT` mechanism (testing.md §3.8.1). testing.md owns the mechanism; the points are
//! placed by their owning docs.
//!
//! ```text
//! INIT_FAULT=<point>[@<n>]:<action>[,<point>[@<n>]:<action>…]
//! action = abort | exit<code> | hang | enospc | sleep<ms>
//! ```
//!
//! `@n` fires on the n-th time the point is reached (default 1). `abort` calls
//! `std::process::abort()`; `exit<code>` exits with that code; `hang` sleeps forever; `sleep<ms>`
//! pauses the calling thread; `enospc` makes [`Plan::fire`] return `io::ErrorKind::StorageFull`,
//! which the caller returns from its next write at that point.
//!
//! This file is std-only and self-contained so the kernel crates (behind their `fault-injection`
//! feature) and `init-root` can use the same grammar: depend on `init-testkit` optionally, or
//! include it with `#[path]`. The fakes (`init-fakebody`, `init-fakekernel`, `init-fakeroot`) always
//! honour it, since they exist only for tests.

use std::collections::BTreeMap;
use std::io;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// What a fault point does when it fires.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Abort,
    Exit(i32),
    Hang,
    Enospc,
    Sleep(u64),
}

impl Action {
    fn parse(s: &str) -> Result<Action, String> {
        Ok(match s {
            "abort" => Action::Abort,
            "hang" => Action::Hang,
            "enospc" => Action::Enospc,
            _ if s.starts_with("exit") => {
                let c: i32 = s[4..]
                    .parse()
                    .map_err(|_| format!("bad exit code in `{s}`"))?;
                if !(0..=255).contains(&c) {
                    return Err(format!("exit code out of range in `{s}`"));
                }
                Action::Exit(c)
            }
            _ if s.starts_with("sleep") => Action::Sleep(
                s[5..]
                    .parse()
                    .map_err(|_| format!("bad sleep ms in `{s}`"))?,
            ),
            _ => {
                return Err(format!(
                    "unknown fault action `{s}` (abort | exit<code> | hang | enospc | sleep<ms>)"
                ));
            }
        })
    }
}

/// One `<point>[@<n>]:<action>` rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    pub point: String,
    pub nth: u64,
    pub action: Action,
}

/// A parsed `INIT_FAULT` value with per-point hit counters.
#[derive(Debug, Default)]
pub struct Plan {
    rules: Vec<Rule>,
    counts: Mutex<BTreeMap<String, u64>>,
}

fn valid_point(p: &str) -> bool {
    !p.is_empty()
        && p.len() <= 128
        && p.bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
}

impl Plan {
    /// Parse the grammar above. An empty string is an empty plan.
    pub fn parse(s: &str) -> Result<Plan, String> {
        let mut rules = Vec::new();
        for item in s.split(',').map(str::trim).filter(|i| !i.is_empty()) {
            let (lhs, action) = item
                .rsplit_once(':')
                .ok_or_else(|| format!("fault `{item}`: missing `:<action>`"))?;
            let (point, nth) = match lhs.split_once('@') {
                Some((p, n)) => (
                    p,
                    n.parse::<u64>()
                        .map_err(|_| format!("fault `{item}`: bad `@<n>`"))?,
                ),
                None => (lhs, 1),
            };
            if nth == 0 {
                return Err(format!(
                    "fault `{item}`: `@0` never fires; counts start at 1"
                ));
            }
            if !valid_point(point) {
                return Err(format!("fault `{item}`: bad point name"));
            }
            rules.push(Rule {
                point: point.to_string(),
                nth,
                action: Action::parse(action)?,
            });
        }
        Ok(Plan {
            rules,
            counts: Mutex::new(BTreeMap::new()),
        })
    }

    /// The plan in `INIT_FAULT`, or an empty plan. An invalid value is reported on stderr and the
    /// process exits 64 (`EX_USAGE`): a test that asked for a fault must not run without it.
    pub fn from_env() -> Plan {
        match std::env::var("INIT_FAULT") {
            Ok(v) => Plan::parse(&v).unwrap_or_else(|e| {
                eprintln!("INIT_FAULT: {e}");
                std::process::exit(64)
            }),
            Err(_) => Plan::default(),
        }
    }

    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Count one arrival at `point` and return the action of a rule that fires on this arrival.
    pub fn hit(&self, point: &str) -> Option<Action> {
        if self.rules.is_empty() {
            return None;
        }
        let n = {
            let mut c = self.counts.lock().unwrap_or_else(|e| e.into_inner());
            let e = c.entry(point.to_string()).or_insert(0);
            *e += 1;
            *e
        };
        self.rules
            .iter()
            .find(|r| r.point == point && r.nth == n)
            .map(|r| r.action)
    }

    /// Reach `point`: perform the action if a rule fires. `enospc` is returned as an error for the
    /// caller's next write; every other action is performed here (or never returns).
    pub fn fire(&self, point: &str) -> io::Result<()> {
        match self.hit(point) {
            None => Ok(()),
            Some(a) => {
                eprintln!("INIT_FAULT: {point} fired: {a:?}");
                perform(a)
            }
        }
    }
}

/// Perform an action (see the module docs).
pub fn perform(a: Action) -> io::Result<()> {
    match a {
        Action::Abort => std::process::abort(),
        Action::Exit(c) => std::process::exit(c),
        Action::Hang => loop {
            std::thread::sleep(Duration::from_secs(3600));
        },
        Action::Sleep(ms) => {
            std::thread::sleep(Duration::from_millis(ms));
            Ok(())
        }
        Action::Enospc => Err(io::Error::new(
            io::ErrorKind::StorageFull,
            "INIT_FAULT enospc",
        )),
    }
}

/// The process-wide plan, read from `INIT_FAULT` once.
pub fn global() -> &'static Plan {
    static P: OnceLock<Plan> = OnceLock::new();
    P.get_or_init(Plan::from_env)
}

/// Shorthand for `global().fire(point)`.
pub fn point(name: &str) -> io::Result<()> {
    global().fire(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grammar() {
        let p = Plan::parse("supervise.handoff.after_paused:sleep2000, gate.g4.mid@3:abort,x:exit75,y:hang,z:enospc").unwrap();
        assert_eq!(
            p.rules(),
            &[
                Rule {
                    point: "supervise.handoff.after_paused".into(),
                    nth: 1,
                    action: Action::Sleep(2000)
                },
                Rule {
                    point: "gate.g4.mid".into(),
                    nth: 3,
                    action: Action::Abort
                },
                Rule {
                    point: "x".into(),
                    nth: 1,
                    action: Action::Exit(75)
                },
                Rule {
                    point: "y".into(),
                    nth: 1,
                    action: Action::Hang
                },
                Rule {
                    point: "z".into(),
                    nth: 1,
                    action: Action::Enospc
                },
            ]
        );
        for bad in [
            "p",
            "p:boom",
            "p@0:abort",
            "p@x:abort",
            ":abort",
            "p:exit999",
            "p:sleepx",
            "a b:abort",
        ] {
            assert!(Plan::parse(bad).is_err(), "{bad}");
        }
        assert!(Plan::parse("").unwrap().is_empty());
    }

    #[test]
    fn nth_arrival_fires_once() {
        let p = Plan::parse("a@2:enospc,b:sleep1").unwrap();
        assert!(p.fire("a").is_ok());
        assert_eq!(p.fire("a").unwrap_err().kind(), io::ErrorKind::StorageFull);
        assert!(p.fire("a").is_ok());
        assert_eq!(p.hit("b"), Some(Action::Sleep(1)));
        assert_eq!(p.hit("b"), None);
        assert_eq!(p.hit("unrelated"), None);
    }
}
