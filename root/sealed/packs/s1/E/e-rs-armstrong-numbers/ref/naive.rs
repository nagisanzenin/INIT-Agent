pub fn is_armstrong_number(num: u64) -> bool {
    let s = format!("{num}");
    let l = s.len();
    s.chars()
        .map(|c| (c.to_digit(10).unwrap() as u64).pow(l as u32))
        .try_fold(0u64, u64::checked_add)
        == Some(num)
}
