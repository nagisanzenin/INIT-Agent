//! provider.md §8 item 11 and the L1 split fuzz: every fixture decodes identically when split at
//! every byte offset, and 10k random / mutated byte streams through `SseReader` + each decoder
//! never panic (fixed seed).

use std::io::BufReader;
use std::time::{Duration, Instant};

use super::sse::SseReader;
use super::sse::tests_support::Trickle;
use super::{Decoder, anthropic, deepseek, openai};
use crate::model::{AssistantMessage, ProviderError, StreamEvent};

fn decoder(family: &str) -> Box<dyn Decoder> {
    match family {
        "deepseek" => Box::new(deepseek::DsDecoder::new("deepseek", "deepseek-flash")),
        "anthropic" => Box::new(anthropic::AnDecoder::new("claude-opus-5-5")),
        _ => Box::new(openai::OaDecoder::new("gpt-6-astra")),
    }
}

fn fixtures() -> Vec<(&'static str, String, Vec<u8>)> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/provider/testdata");
    let mut out = Vec::new();
    for fam in ["deepseek", "anthropic", "openai"] {
        let mut names: Vec<_> = std::fs::read_dir(root.join(fam))
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        names.sort();
        for p in names {
            out.push((
                fam,
                p.file_name().unwrap().to_string_lossy().into_owned(),
                std::fs::read(&p).unwrap(),
            ));
        }
    }
    out
}

/// Feeds a stream read `n` bytes at a time; returns a printable summary of everything decoded.
fn run(fam: &str, bytes: &[u8], n: usize) -> String {
    let mut d = decoder(fam);
    let mut r = SseReader::new(BufReader::with_capacity(n, Trickle { data: bytes, n }));
    let mut evs: Vec<StreamEvent> = Vec::new();
    let res: Result<AssistantMessage, ProviderError> = loop {
        match r.next_event() {
            Ok(Some(ev)) => {
                if let Err(e) = d.feed(&ev, &mut evs) {
                    break Err(e);
                }
                if d.finished() {
                    break d.finish();
                }
            }
            Ok(None) => {
                break if d.finished() {
                    d.finish()
                } else {
                    Err(ProviderError::Truncated)
                };
            }
            Err(e) => break Err(e),
        }
    };
    let partial = d.partial();
    format!("{evs:?}\n{res:?}\n{partial:?}")
}

#[test]
fn every_fixture_splits_at_every_byte() {
    let fx = fixtures();
    assert!(fx.len() >= 15);
    for (fam, name, bytes) in &fx {
        let whole = run(fam, bytes, bytes.len().max(1));
        for n in [1, 2, 3, 5, 8, 13, 64] {
            assert_eq!(run(fam, bytes, n), whole, "{fam}/{name} n={n}");
        }
    }
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

const TOKENS: [&[u8]; 14] = [
    b"\n",
    b"\r\n",
    b"\r",
    b"\n\n",
    b"data: ",
    b"event: ",
    b": keep-alive\n",
    b"{",
    b"}",
    b"[DONE]",
    b"\"index\":",
    b"\"tool_calls\":[{",
    b"\xef\xbb\xbf",
    b"\xff",
];

fn mutate(rng: &mut Rng, fx: &[(&'static str, String, Vec<u8>)]) -> (&'static str, Vec<u8>) {
    let (fam, _, base) = &fx[rng.below(fx.len())];
    let mut b = base.clone();
    match rng.below(6) {
        0 => {
            let n = rng.below(512);
            b = (0..n).map(|_| rng.next() as u8).collect();
        }
        1 => {
            for _ in 0..1 + rng.below(8) {
                if !b.is_empty() {
                    let i = rng.below(b.len());
                    b[i] = rng.next() as u8;
                }
            }
        }
        2 => b.truncate(rng.below(b.len() + 1)),
        3 => {
            for _ in 0..1 + rng.below(6) {
                let i = rng.below(b.len() + 1);
                let t = TOKENS[rng.below(TOKENS.len())];
                b.splice(i..i, t.iter().copied());
            }
        }
        4 => {
            let (_, _, other) = &fx[rng.below(fx.len())];
            let i = rng.below(b.len() + 1);
            let j = rng.below(other.len() + 1);
            b.truncate(i);
            b.extend_from_slice(&other[j..]);
        }
        _ => {
            if !b.is_empty() {
                let i = rng.below(b.len());
                let j = (i + rng.below(64)).min(b.len());
                b.drain(i..j);
            }
        }
    }
    (fam, b)
}

#[test]
fn fuzz_10k_streams_no_panic() {
    let fx = fixtures();
    let mut rng = Rng(0x1A2B_3C4D_5E6F_7081);
    let t0 = Instant::now();
    for i in 0..10_000 {
        let (fam, bytes) = mutate(&mut rng, &fx);
        // Each stream also goes through a decoder of another family now and then.
        let fam = if i % 7 == 0 {
            ["deepseek", "anthropic", "openai"][rng.below(3)]
        } else {
            fam
        };
        let n = 1 + rng.below(4096);
        let _ = run(fam, &bytes, n);
    }
    let took = t0.elapsed();
    eprintln!("fuzz: 10k streams in {took:?}");
    // provider.md asks for < 2 s; debug builds get headroom.
    assert!(
        took < Duration::from_secs(if cfg!(debug_assertions) { 20 } else { 2 }),
        "{took:?}"
    );
}
