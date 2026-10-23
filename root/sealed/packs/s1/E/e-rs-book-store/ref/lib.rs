// Reference solution of e-rs-book-store (rendered by gen/run with the seeded constants).
use std::collections::HashMap;

const TITLES: usize = @TITLES@;
const UNIT_PRICE: u32 = @UNIT_PRICE@;
/// DISCOUNT[g] = percent off for a group of g different titles (index 0 unused).
const DISCOUNT: [u32; TITLES + 1] = [@DISCOUNT@];

fn group_price(g: usize) -> u32 {
    g as u32 * UNIT_PRICE * (100 - DISCOUNT[g]) / 100
}

fn best(counts: Vec<u32>, memo: &mut HashMap<Vec<u32>, u32>) -> u32 {
    if counts.is_empty() {
        return 0;
    }
    if let Some(&v) = memo.get(&counts) {
        return v;
    }
    let n = counts.len();
    let mut result = u32::MAX;
    for mask in 1u32..(1u32 << n) {
        let mut next = counts.clone();
        for (i, c) in next.iter_mut().enumerate() {
            if mask & (1 << i) != 0 {
                *c -= 1;
            }
        }
        let mut next: Vec<u32> = next.into_iter().filter(|&c| c > 0).collect();
        next.sort_unstable_by(|a, b| b.cmp(a));
        let cost = group_price(mask.count_ones() as usize) + best(next, memo);
        result = result.min(cost);
    }
    memo.insert(counts, result);
    result
}

/// The lowest price, in cents, of the basket (each entry is a title number 1..=TITLES).
pub fn lowest_price(books: &[u32]) -> u32 {
    let mut counts = vec![0u32; TITLES];
    for &b in books {
        counts[(b - 1) as usize] += 1;
    }
    let mut counts: Vec<u32> = counts.into_iter().filter(|&c| c > 0).collect();
    counts.sort_unstable_by(|a, b| b.cmp(a));
    best(counts, &mut HashMap::new())
}
