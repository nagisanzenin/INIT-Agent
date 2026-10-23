//! `INIT_FAULT=<point>[@<n>]:<action>[,…]` (testing.md §3.8.1) for the Root's lineage, mirror and
//! `kernel.git` points (root/root.md §4.7). Compiled only with the `fault-injection` feature (testing.md §3.8.1;
//! WP23 D3), which `test-fixtures` turns on; release builds have neither.

use std::collections::HashMap;
use std::io;
use std::sync::Mutex;
use std::time::Duration;

/// Arrivals at the points outside a `Lineage` (mirror, `kernel.git`); a `Lineage` keeps its own.
pub static HITS: Mutex<Option<HashMap<String, u32>>> = Mutex::new(None);

/// The action when `point` fires now (its `@n`-th arrival in `hits`, default the first), else None.
pub fn fires(spec: &str, point: &str, hits: &mut HashMap<String, u32>) -> Option<String> {
    let (n, action) = spec.split(',').find_map(|s| {
        let (p, a) = s.split_once(':')?;
        let (name, n) = p.split_once('@').map_or((p, 1), |(x, n)| (x, n.parse().unwrap_or(1)));
        (name == point).then(|| (n, a.to_string()))
    })?;
    let c = hits.entry(point.to_string()).or_insert(0);
    *c += 1;
    (*c == n).then_some(action)
}

pub fn act(a: &str) -> io::Result<()> {
    match a {
        "abort" => std::process::abort(),
        "enospc" => Err(io::Error::from(io::ErrorKind::StorageFull)),
        "hang" => loop {
            std::thread::sleep(Duration::from_secs(60));
        },
        _ if a.starts_with("exit") => std::process::exit(a[4..].parse().unwrap_or(1)),
        _ if a.starts_with("sleep") => {
            std::thread::sleep(Duration::from_millis(a[5..].parse().unwrap_or(0)));
            Ok(())
        }
        _ => Ok(()),
    }
}
