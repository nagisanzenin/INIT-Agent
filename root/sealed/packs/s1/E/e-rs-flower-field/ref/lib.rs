// Reference solution of e-rs-flower-field (rendered by gen/run with the seeded constants).
const FLOWER: char = @FLOWER@;
const ZERO: char = @ZERO@;
const OFFSETS: &[(i64, i64)] = &[@OFFSETS@];
const WRAP: bool = @WRAP@;

/// Annotates every non-flower cell with the number of flowers among its neighbours.
pub fn annotate(garden: &[&str]) -> Vec<String> {
    let grid: Vec<Vec<char>> = garden.iter().map(|row| row.chars().collect()).collect();
    let h = grid.len() as i64;
    let mut out = Vec::with_capacity(grid.len());
    for (r, row) in grid.iter().enumerate() {
        let w = row.len() as i64;
        let mut line = String::new();
        for (c, &cell) in row.iter().enumerate() {
            if cell == FLOWER {
                line.push(FLOWER);
                continue;
            }
            let mut seen: Vec<(i64, i64)> = Vec::new();
            for &(dr, dc) in OFFSETS {
                let (mut nr, mut nc) = (r as i64 + dr, c as i64 + dc);
                if WRAP {
                    nr = nr.rem_euclid(h);
                    nc = nc.rem_euclid(w);
                } else if nr < 0 || nr >= h || nc < 0 || nc >= w {
                    continue;
                }
                if (nr, nc) == (r as i64, c as i64) || seen.contains(&(nr, nc)) {
                    continue;
                }
                seen.push((nr, nc));
            }
            let count = seen.iter().filter(|&&(nr, nc)| grid[nr as usize][nc as usize] == FLOWER).count();
            line.push(if count == 0 { ZERO } else { char::from(b'0' + count as u8) });
        }
        out.push(line);
    }
    out
}
