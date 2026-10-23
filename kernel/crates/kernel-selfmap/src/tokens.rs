//! The token measure (selfmap.md §3.4): `o200k_base` BPE via tiktoken-rs 0.12.1,
//! `CoreBPE::encode_ordinary(text).len()` on the process-wide singleton. "Ordinary" means special-token
//! strings in a file count as plain text. The count is a measure, not the bill.

use tiktoken_rs::CoreBPE;

/// Files larger than this are not tokenized (`tokens: null`, with a warning; selfmap.md §3.2).
pub const MAX_TOKENIZE_BYTES: usize = 1 << 20;

/// The fixed, model-independent tokenizer.
#[derive(Clone, Copy)]
pub struct Tokenizer {
    bpe: &'static CoreBPE,
}

impl Tokenizer {
    /// The `o200k_base` singleton (initialized once per process).
    pub fn o200k() -> Self {
        Tokenizer {
            bpe: tiktoken_rs::o200k_base_singleton(),
        }
    }

    /// Number of `o200k_base` tokens in `text`.
    pub fn count(&self, text: &str) -> u64 {
        self.bpe.encode_ordinary(text).len() as u64
    }
}

impl std::fmt::Debug for Tokenizer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Tokenizer(o200k_base)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_counts() {
        let t = Tokenizer::o200k();
        assert_eq!(t.count(""), 0);
        // o200k_base: "hello world" = [24912, 2375]; special-token text counts as ordinary text.
        assert_eq!(
            tiktoken_rs::o200k_base_singleton().encode_ordinary("hello world"),
            vec![24912, 2375]
        );
        assert_eq!(t.count("hello world"), 2);
        assert!(t.count("<|endoftext|>") > 1);
    }
}
