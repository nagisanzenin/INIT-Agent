//! v0.5.3 L6: the meter's property test and its golden fixtures per codec.
//!
//! - The property: over every combination of usage classes (uncached input, cached input, cache write, output,
//!   reasoning, a class without a price), cut or not, on the catalog's models and on random charter prices, the
//!   booked bill is at most the reservation and never below the provider's usage at list price.
//! - The golden fixtures (`tap/testdata/`): real-shaped streams of each codec (DeepSeek's are real captures from
//!   seed/src/provider/testdata/deepseek/real-*.sse; Anthropic's and OpenAI's follow the documented shapes of the
//!   seed's fixtures), with the usage, the hold and the bill a separate reference computed by hand from each
//!   provider's usage semantics (`golden.json`).

use super::classes::{self, Model};
use super::money::{self, book, dearest, reservation};
use super::tap::{Family, Tap, Usage};
use serde_json::Value;

/// The catalog row of `provider`/`model` as an install's Root prices it: `providers add`'s charter fragment through
/// the charter rules (C11) and the egress compile.
fn catalog_model(provider: &str, model: &str) -> Model {
    let r = crate::providers::AddReq { preset: provider.into(), model: Some(model.into()), ..Default::default() };
    let (_, md) = crate::providers::charter_fragment(&crate::providers::spec(&r).unwrap());
    let mut models = toml::Table::new();
    models.insert(model.into(), toml::Value::Table(md));
    let mut class = toml::Table::new();
    class.insert("models".into(), toml::Value::Table(models));
    let mut user = toml::Table::new();
    user.insert("providers".into(), toml::Value::Table(class));
    let mut e = Vec::new();
    let t = classes::effective_actions(Some(&toml::Value::Table(user)), false, &mut e);
    assert!(e.is_empty(), "{e:?}");
    classes::compile(&t)["providers"].models[model].clone()
}

/// Every model the catalog prices.
fn catalog() -> Vec<(String, Model)> {
    crate::providers::CATALOG.iter().filter(|p| p.host.is_some()).flat_map(|p| p.models.iter().map(move |m| (p.id, m.id)))
        .map(|(p, m)| (format!("{p}/{m}"), catalog_model(p, m))).collect()
}

/// xorshift64*: a fixed-seed generator, so a failure reproduces.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    /// Uniform in `0..=n`, with the ends drawn often (the edges are where meters break).
    fn upto(&mut self, n: u64) -> u64 {
        match self.next() % 8 {
            0 => 0,
            1 => n,
            _ => self.next() % n.saturating_add(1).max(1),
        }
    }
}

/// The reference list price: each class at its price, rounded up once (u128, independent of `tap::musd`).
fn list(parts: &[(u64, u64)]) -> u64 {
    let s: u128 = parts.iter().map(|&(t, p)| u128::from(t) * u128::from(p)).sum();
    u64::try_from(s.div_ceil(1_000_000)).unwrap_or(u64::MAX)
}

/// A random charter price table (µUSD per 1M tokens), cache prices up to 25× the input (above the 2× the hold's
/// default assumes) and the classes sometimes free.
fn random_model(r: &mut Rng) -> Model {
    let input = r.upto(20_000_000);
    Model { min_level: 0, input, cache_read: r.upto(input.max(1) * 3), cache_write: r.upto(input.max(1) * 25), cache_write_1h: r.upto(input.max(1) * 25),
        output: r.upto(100_000_000), reserve_out: 1 }
}

#[test]
fn the_bill_is_within_the_hold_and_never_under_the_list_price() {
    let mut r = Rng(0x1A57_0C0D_E5EE_D001);
    let mut models = catalog();
    assert!(models.len() >= 6, "the catalog's six models: {:?}", models.iter().map(|m| &m.0).collect::<Vec<_>>());
    for i in 0..40 {
        models.push((format!("random-{i}"), random_model(&mut r)));
    }
    let mut cases = 0u64;
    for (name, md) in &models {
        // the eight classes: uncached input, cached input, cache write (5 min or 1 h), output, reasoning, no price,
        // (v0.5.4) a write TTL without a price inside the input (a 12-hour write in the provider's write breakdown), and
        // (review 4, n1) a write the provider did not split by TTL
        for combo in 0u32..256 {
            let has = |k: u32| combo & (1 << k) != 0;
            for _ in 0..12 {
                let bytes = 1 + r.upto(2_000_000);
                let max_out = 1 + r.upto(money::MAX_OUT_TOKENS - 1);
                let hold = reservation(bytes, Some(max_out), md).0;
                // the reservation's domain: at least two request bytes per input token, the output within the cap
                let mut room = bytes / 2;
                let mut take = |on: bool, r: &mut Rng| if on { let t = r.upto(room); room -= t; t } else { 0 };
                let (uncached, cached) = (take(has(0), &mut r), take(has(1), &mut r));
                let write = take(has(2), &mut r);
                let uw = take(has(6), &mut r);
                let us = take(has(7), &mut r);
                let w1 = if r.next().is_multiple_of(2) { write } else { 0 };
                let mut out_room = max_out;
                let mut take_out = |on: bool, r: &mut Rng| if on { let t = r.upto(out_room); out_room -= t; t } else { 0 };
                let (visible, reasoning) = (take_out(has(3), &mut r), take_out(has(4), &mut r));
                let unknown = if has(5) { 1 + r.upto(bytes / 2) } else { 0 } + uw;
                let output = visible + reasoning; // every family reports reasoning inside its output count
                let base = Usage { input: uncached + cached + write + uw + us, cache_read: cached, cache_write: write - w1, cache_write_1h: w1,
                    cache_write_unsplit: us, output, unknown, unknown_write: uw, ..Usage::default() };
                // v0.5.4 (review 4, n8): at the dearer of the two write prices, as `Usage::cost` prices it; an unsplit write too (n1)
                let wd = md.cache_write.max(md.cache_write_1h);
                let priced = list(&[(uncached, md.input), (cached, md.cache_read), (write - w1, md.cache_write), (w1, md.cache_write_1h), (uw, wd), (us, wd),
                    (output, md.output)]);
                for partial in [false, true] {
                    let u = Usage { partial, ..base };
                    let (bill, flags) = book(Some(&u), md, hold, max_out, true);
                    let ctx = || format!("{name} combo {combo:06b} partial {partial}: {u:?} bytes {bytes} max_out {max_out} hold {hold} bill {bill}");
                    assert!(bill <= hold, "over the hold: {}", ctx());
                    assert!(bill >= priced, "under the list price: {}", ctx());
                    if partial {
                        // the provider may have generated up to the cap: any actual output is covered
                        let worst = list(&[(uncached, md.input), (cached, md.cache_read), (write - w1, md.cache_write), (w1, md.cache_write_1h), (uw, wd), (us, wd),
                            (max_out, md.output)]);
                        assert!(bill >= worst, "a cut stream under its cap's price: {}", ctx());
                    }
                    if unknown > 0 {
                        let out = if partial { max_out } else { output };
                        // every token once: the write TTL without a price is in `unknown`, not in an input class too
                        let at_dearest = list(&[(uncached, md.input), (cached, md.cache_read), (write - w1, md.cache_write), (w1, md.cache_write_1h),
                            (us, wd), (out, md.output), (unknown, dearest(md))]);
                        let floor = list(&[(uncached, md.input), (cached, md.cache_read), (write - w1, md.cache_write), (w1, md.cache_write_1h), (uw, wd),
                            (us, wd), (out, md.output)]);
                        assert!(bill >= at_dearest.min(hold), "a class without a price under the dearest price: {}", ctx());
                        assert_eq!(bill, floor.max(at_dearest.min(hold)), "a token billed twice, or not at the dearest price: {}", ctx());
                        assert!(flags.contains(&"price_drift"), "{}", ctx());
                    }
                    assert_eq!(flags.contains(&"usage_partial"), partial, "{}", ctx());
                    cases += 1;
                }
            }
        }
        // outside the domain (denser than the hold assumes, or more than reserved): still never under the list price
        for _ in 0..200 {
            let u = Usage { input: r.upto(u32::MAX as u64), cache_read: r.upto(1 << 30), cache_write: r.upto(1 << 30), cache_write_1h: r.upto(1 << 30),
                output: r.upto(1 << 24), ..Usage::default() };
            let cr = u.cache_read.min(u.input);
            let w1 = u.cache_write_1h.min(u.input - cr);
            let w5 = u.cache_write.min(u.input - cr - w1);
            let priced = list(&[(u.input - cr - w1 - w5, md.input), (cr, md.cache_read), (w5, md.cache_write), (w1, md.cache_write_1h), (u.output, md.output)]);
            assert_eq!(book(Some(&u), md, 1, 1, true).0, priced, "{name}: {u:?} is booked at its list price, above any hold");
        }
    }
    // no usage: the hold on a 2xx answer, nothing on a refusal
    assert_eq!(book(None, &models[0].1, 12_345, 1, true).0, 12_345);
    assert_eq!(book(None, &models[0].1, 12_345, 1, false).0, 0);
    assert!(cases >= 46 * 256 * 12 * 2, "{cases}");
}

/// The uncached-only case the hold covers at one token per byte (a token is at least one byte).
#[test]
fn an_uncached_prompt_of_one_token_per_byte_fits_the_hold() {
    for (name, md) in catalog() {
        for (bytes, max_out) in [(1u64, 1u64), (300_000, 32_768), (2_000_000, 65_536)] {
            let u = Usage { input: bytes, output: max_out, ..Usage::default() };
            let hold = reservation(bytes, Some(max_out), &md).0;
            assert!(book(Some(&u), &md, hold, max_out, true).0 <= hold, "{name}: {bytes} bytes");
        }
    }
}

#[test]
fn golden_fixtures_per_codec() {
    let golden: Value = serde_json::from_str(include_str!("tap/testdata/golden.json")).unwrap();
    let files: [(&str, &str); 13] = [
        ("deepseek-cache-hit.sse", include_str!("tap/testdata/deepseek-cache-hit.sse")),
        ("deepseek-reasoning-tool.sse", include_str!("tap/testdata/deepseek-reasoning-tool.sse")),
        ("deepseek-cut.sse", include_str!("tap/testdata/deepseek-cut.sse")),
        ("deepseek-drift.sse", include_str!("tap/testdata/deepseek-drift.sse")),
        ("anthropic-cache-write-ttl.sse", include_str!("tap/testdata/anthropic-cache-write-ttl.sse")),
        ("anthropic-cache-read.sse", include_str!("tap/testdata/anthropic-cache-read.sse")),
        ("anthropic-cut-after-start.sse", include_str!("tap/testdata/anthropic-cut-after-start.sse")),
        ("anthropic-drift.sse", include_str!("tap/testdata/anthropic-drift.sse")),
        ("openai-cache-write.sse", include_str!("tap/testdata/openai-cache-write.sse")),
        // v0.5.4: the live qualify's request 1, a 30-minute write (OpenAI's default since 2026-10-20), at the write price
        ("openai-cache-write-30m.sse", include_str!("tap/testdata/openai-cache-write-30m.sse")),
        ("openai-cache-read.sse", include_str!("tap/testdata/openai-cache-read.sse")),
        ("openai-cut.sse", include_str!("tap/testdata/openai-cut.sse")),
        ("openai-drift.sse", include_str!("tap/testdata/openai-drift.sse")),
    ];
    assert_eq!(golden.as_object().unwrap().len(), files.len(), "one golden entry per fixture");
    for (file, text) in files {
        let g = &golden[file];
        let (provider, family) = match file.split('-').next() {
            Some("deepseek") => ("deepseek", Family::Chat),
            Some("anthropic") => ("anthropic", Family::Messages),
            _ => ("openai", Family::Responses),
        };
        let md = catalog_model(provider, g["model"].as_str().unwrap());
        let (bytes, max_out) = (g["req_bytes"].as_u64().unwrap(), g["max_out"].as_u64().unwrap());
        let hold = reservation(bytes, Some(max_out), &md).0;
        assert_eq!(hold, g["hold_musd"].as_u64().unwrap(), "{file}: the hold");
        for split in [1, 7, 64, 100_000] {
            let mut t = Tap::new(family, true);
            for c in text.as_bytes().chunks(split) {
                t.feed(c);
            }
            let (usage, finish) = t.finish();
            assert_eq!(usage.map(|u| u.json()).unwrap_or(Value::Null), g["usage"], "{file} at {split}: the usage");
            assert_eq!(finish.map(Value::from).unwrap_or(Value::Null), g["finish"], "{file} at {split}: the finish");
            let (bill, flags) = book(usage.as_ref(), &md, hold, max_out, true);
            assert_eq!(bill, g["bill_musd"].as_u64().unwrap(), "{file} at {split}: the bill");
            assert_eq!(serde_json::json!(flags), g["flags"], "{file} at {split}: the flags");
            assert!(bill <= hold, "{file}: within the hold");
            if let Some(l) = g["list_musd"].as_u64() {
                assert!(bill >= l, "{file}: never under the provider's usage at list price");
            }
        }
    }
}
