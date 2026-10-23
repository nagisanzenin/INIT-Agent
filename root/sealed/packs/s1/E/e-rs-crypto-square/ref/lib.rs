// Reference solution of e-rs-crypto-square (rendered by gen/run with the seeded constants).
const KEEP_DIGITS: bool = @KEEP_DIGITS@;
const UPPER: bool = @UPPER@;
const TALL: bool = @TALL@;
const PAD: char = @PAD@;
const SEPARATOR: &str = @SEPARATOR@;
const REVERSE_CHUNKS: bool = @REVERSE_CHUNKS@;

fn shape(n: usize) -> (usize, usize) {
    // (rows, columns)
    let mut big = 0;
    while big * big < n {
        big += 1;
    }
    let small = if big * (big - 1) >= n { big - 1 } else { big };
    if TALL { (big, small) } else { (small, big) }
}

/// Encodes `input` with the square code of the task.
pub fn encrypt(input: &str) -> String {
    let text: Vec<char> = input
        .chars()
        .filter(|c| c.is_ascii_alphabetic() || (KEEP_DIGITS && c.is_ascii_digit()))
        .map(|c| if UPPER { c.to_ascii_uppercase() } else { c.to_ascii_lowercase() })
        .collect();
    if text.is_empty() {
        return String::new();
    }
    let (rows, cols) = shape(text.len());
    let mut grid = text.clone();
    grid.resize(rows * cols, PAD);
    let mut chunks: Vec<String> = Vec::with_capacity(cols);
    for c in 0..cols {
        let mut chunk: Vec<char> = (0..rows).map(|r| grid[r * cols + c]).collect();
        if REVERSE_CHUNKS {
            chunk.reverse();
        }
        chunks.push(chunk.into_iter().collect());
    }
    chunks.join(SEPARATOR)
}
