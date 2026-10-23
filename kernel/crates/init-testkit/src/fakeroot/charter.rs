//! The charter values the fake Root reports (root/root.md §3.12; root/egress.md §3.3, §3.5).
//!
//! The fake is not a validator: it reads a charter fixture leniently (unknown keys ignored, missing
//! keys defaulted to the §3.12.2 template values) and takes only what the kernel-facing contract
//! reports: the level cap, money ceilings, quotas, the model table and the heartbeat interval.
//! `init-root`'s own charter tests (WP20) own validation.

use serde_json::{Value, json};
use std::path::Path;

/// One model of the `providers` class (peak prices, µUSD per million tokens).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Model {
    pub provider: String,
    pub model: String,
    pub min_level: u8,
    pub input_musd_per_mtok: u64,
    pub output_musd_per_mtok: u64,
    pub reserve_output_tokens: u64,
}

/// What the fake Root needs from a charter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Charter {
    /// Hex SHA-256 of the fixture bytes (of the empty string when built in).
    pub digest: String,
    pub max_level: u8,
    pub day_limit_musd: u64,
    pub total_limit_musd: u64,
    pub sealed_reserve_musd: u64,
    pub day_offset_min: i64,
    pub max_instances: u32,
    pub parallel_bodies: u32,
    pub models: Vec<Model>,
    pub hb_interval_s: u64,
    pub test_mode: bool,
    /// ADR-011 `[extend] mode`: `"free"` or `"gated"` (the default).
    pub extend_mode: String,
}

/// Parse a decimal money string (`"5.00"`) exactly into micro-USD.
pub fn parse_money(s: &str) -> Option<u64> {
    let s = s.trim();
    let (int, frac) = s.split_once('.').unwrap_or((s, ""));
    if int.is_empty()
        || !int.bytes().all(|c| c.is_ascii_digit())
        || frac.len() > 6
        || !frac.bytes().all(|c| c.is_ascii_digit())
    {
        return None;
    }
    let mut f = frac.to_string();
    while f.len() < 6 {
        f.push('0');
    }
    int.parse::<u64>()
        .ok()?
        .checked_mul(1_000_000)?
        .checked_add(f.parse::<u64>().ok()?)
}

fn deepseek_flash() -> Model {
    Model {
        provider: "deepseek".into(),
        model: "deepseek-flash".into(),
        min_level: 0,
        input_musd_per_mtok: 300_000,
        output_musd_per_mtok: 1_200_000,
        reserve_output_tokens: 65_536,
    }
}

impl Default for Charter {
    /// The §3.12.2 template values (D13), in test mode.
    fn default() -> Self {
        Charter {
            digest: crate::util::sha256_hex(b""),
            max_level: 4,
            day_limit_musd: 5_000_000,
            total_limit_musd: 25_000_000,
            sealed_reserve_musd: 0,
            day_offset_min: 0,
            max_instances: 12,
            parallel_bodies: 4,
            models: vec![deepseek_flash()],
            hb_interval_s: 5,
            test_mode: true,
            extend_mode: "gated".into(),
        }
    }
}

impl Charter {
    /// Read a TOML charter fixture.
    pub fn load(path: &Path) -> Result<Charter, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let text = String::from_utf8(bytes.clone())
            .map_err(|_| format!("{}: not UTF-8", path.display()))?;
        let mut c = Charter::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        c.digest = crate::util::sha256_hex(&bytes);
        Ok(c)
    }

    /// Parse TOML text (lenient; see the module docs).
    pub fn parse(text: &str) -> Result<Charter, String> {
        let t: toml::Table = text.parse().map_err(|e: toml::de::Error| e.to_string())?;
        let mut c = Charter {
            digest: crate::util::sha256_hex(text.as_bytes()),
            ..Charter::default()
        };
        let get = |path: &[&str]| -> Option<&toml::Value> {
            let mut cur = t.get(path[0])?;
            for k in &path[1..] {
                cur = cur.as_table()?.get(*k)?;
            }
            Some(cur)
        };
        let int = |path: &[&str]| get(path).and_then(|v| v.as_integer());
        if let Some(x) = int(&["autonomy", "max_level"]) {
            c.max_level = u8::try_from(x)
                .ok()
                .filter(|l| *l <= 4)
                .ok_or("autonomy.max_level must be 0..=4")?;
        }
        if let Some(s) = get(&["money", "day_usd"]).and_then(|v| v.as_str()) {
            c.day_limit_musd = parse_money(s).ok_or("money.day_usd: not a decimal amount")?;
        }
        if let Some(s) = get(&["money", "total_usd"]).and_then(|v| v.as_str()) {
            c.total_limit_musd = parse_money(s).ok_or("money.total_usd: not a decimal amount")?;
        }
        if let Some(s) = get(&["money", "day_starts_at_utc_offset"]).and_then(|v| v.as_str()) {
            c.day_offset_min =
                crate::util::parse_offset(s).ok_or("money.day_starts_at_utc_offset: not ±HH:MM")?;
        }
        if let Some(s) = get(&["sealed", "reserve_usd"]).and_then(|v| v.as_str()) {
            c.sealed_reserve_musd =
                parse_money(s).ok_or("sealed.reserve_usd: not a decimal amount")?;
        }
        if let Some(x) = int(&["quotas", "max_instances"]) {
            c.max_instances = u32::try_from(x).map_err(|_| "quotas.max_instances")?;
        }
        if let Some(x) = int(&["quotas", "parallel_bodies"]) {
            c.parallel_bodies = u32::try_from(x).map_err(|_| "quotas.parallel_bodies")?;
        }
        if let Some(x) = int(&["root", "hb_interval_s"]) {
            c.hb_interval_s = u64::try_from(x).map_err(|_| "root.hb_interval_s")?.max(1);
        }
        if let Some(b) = get(&["root", "test_mode"]).and_then(|v| v.as_bool()) {
            c.test_mode = b;
        }
        if let Some(m) = get(&["extend", "mode"]).and_then(|v| v.as_str()) {
            if !matches!(m, "free" | "gated") {
                return Err("extend.mode must be \"free\" or \"gated\"".into());
            }
            c.extend_mode = m.to_string();
        }
        if let Some(models) = get(&["actions", "providers", "models"]).and_then(|v| v.as_table()) {
            c.models.clear();
            for (name, m) in models {
                let m = m
                    .as_table()
                    .ok_or_else(|| format!("actions.providers.models.{name}: not a table"))?;
                let i = |k: &str, d: u64| {
                    m.get(k)
                        .and_then(|v| v.as_integer())
                        .map(|x| x.max(0) as u64)
                        .unwrap_or(d)
                };
                c.models.push(Model {
                    provider: m
                        .get("provider")
                        .and_then(|v| v.as_str())
                        .unwrap_or("deepseek")
                        .to_string(),
                    model: name.clone(),
                    min_level: i("min_level", 0).min(4) as u8,
                    input_musd_per_mtok: i("input_musd_per_mtok", 0),
                    output_musd_per_mtok: i("output_musd_per_mtok", 0),
                    reserve_output_tokens: i("reserve_output_tokens", 65_536),
                });
            }
        }
        Ok(c)
    }

    /// The model entry for a request's `model`.
    pub fn model(&self, name: &str) -> Option<&Model> {
        self.models.iter().find(|m| m.model == name)
    }

    /// The `models` block of `GET /_root/ceilings` at `level` (root/egress.md §4.2).
    pub fn models_json(&self, level: u8) -> Value {
        Value::Array(
            self.models
                .iter()
                .map(|m| {
                    json!({"provider": m.provider, "model": m.model, "min_level": m.min_level,
                           "input_musd_per_mtok": m.input_musd_per_mtok, "output_musd_per_mtok": m.output_musd_per_mtok,
                           "routable": m.min_level <= level})
                })
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn money() {
        assert_eq!(parse_money("5.00"), Some(5_000_000));
        assert_eq!(parse_money("25"), Some(25_000_000));
        assert_eq!(parse_money("0.000001"), Some(1));
        for bad in ["", "1.0000001", "-1", "1e3", "x"] {
            assert_eq!(parse_money(bad), None, "{bad}");
        }
    }

    #[test]
    fn template_values_and_fixture() {
        let c = Charter::parse(
            "schema_version = 1\n[autonomy]\nmax_level = 2\n[money]\nday_usd = \"0.10\"\ntotal_usd = \"1.00\"\n\
             day_starts_at_utc_offset = \"+07:00\"\n[quotas]\nparallel_bodies = 2\nmax_instances = 5\n[root]\nhb_interval_s = 1\n\
             [actions.providers.models.\"mock-frontier-1\"]\nmin_level = 0\ninput_musd_per_mtok = 1000\noutput_musd_per_mtok = 2000\n",
        )
        .unwrap();
        assert_eq!(
            (
                c.max_level,
                c.day_limit_musd,
                c.total_limit_musd,
                c.day_offset_min
            ),
            (2, 100_000, 1_000_000, 420)
        );
        assert_eq!(
            (c.parallel_bodies, c.max_instances, c.hb_interval_s),
            (2, 5, 1)
        );
        assert_eq!(
            c.model("mock-frontier-1").unwrap().output_musd_per_mtok,
            2000
        );
        assert!(c.model("deepseek-flash").is_none());
        assert!(Charter::parse("[autonomy]\nmax_level = 9\n").is_err());
        let d = Charter::default();
        assert_eq!(
            (d.day_limit_musd, d.total_limit_musd, d.max_level),
            (5_000_000, 25_000_000, 4)
        );
    }
}
