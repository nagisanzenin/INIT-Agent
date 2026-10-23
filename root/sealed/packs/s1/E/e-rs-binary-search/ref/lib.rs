// Reference solution of e-rs-binary-search (rendered by gen/run with the seeded constants).
use std::cmp::Ordering;

const ORDER: &str = "@ORDER@"; // "ascending", "descending" or "abs"
const RIGHTMOST: bool = @RIGHTMOST@;

fn cmp(a: i64, b: i64) -> Ordering {
    match ORDER {
        "ascending" => a.cmp(&b),
        "descending" => b.cmp(&a),
        _ => (a.unsigned_abs(), a).cmp(&(b.unsigned_abs(), b)),
    }
}

/// Finds `key` in `array`, which is sorted by the task's order.
pub fn locate(array: &[i64], key: i64) -> Result<usize, usize> {
    let lo = array.partition_point(|&x| cmp(x, key) == Ordering::Less);
    let hi = array.partition_point(|&x| cmp(x, key) != Ordering::Greater);
    if lo == hi {
        Err(lo)
    } else if RIGHTMOST {
        Ok(hi - 1)
    } else {
        Ok(lo)
    }
}
