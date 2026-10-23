//! semver-sort: sort version strings by SemVer 2.0.0 precedence (semver.org §11).
//!
//! A pure-compute T1 extension (WIT init:ext@1, world `tool`), written from the seed's
//! ext-authoring template: no WASI capability is used. The A2 acceptance fixture (testing.md §3.9).
#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]

use serde::Deserialize;
use serde_json::json;
use std::cmp::Ordering;

const NAME: &str = "semver-sort";
const VERSION: &str = env!("CARGO_PKG_VERSION");

const DESCRIPTION: &str = "Sort version strings by SemVer 2.0.0 precedence (lowest first; build \
metadata is ignored, ties keep their input order). Returns {\"sorted\": [...]}.";

const SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "versions": {"type": "array", "items": {"type": "string"}, "description": "SemVer strings, e.g. \"1.0.0-rc.1\""}
  },
  "required": ["versions"],
  "additionalProperties": false
}"#;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    versions: Vec<String>,
}

/// One pre-release identifier: numeric identifiers sort below alphanumeric ones.
#[derive(Debug, PartialEq, Eq)]
enum Ident {
    Num(u64),
    Alpha(String),
}

#[derive(Debug, PartialEq, Eq)]
struct Version {
    core: [u64; 3],
    pre: Vec<Ident>,
}

fn numeric(s: &str) -> Option<u64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) || (s.len() > 1 && s.starts_with('0'))
    {
        return None;
    }
    s.parse().ok()
}

fn parse(v: &str) -> Result<Version, String> {
    let bad = || format!("not a SemVer 2.0.0 version: {v:?}");
    let no_build = v.split_once('+').map_or(v, |(a, _)| a);
    let (core, pre) = match no_build.split_once('-') {
        Some((c, p)) => (c, Some(p)),
        None => (no_build, None),
    };
    let nums: Vec<u64> = core
        .split('.')
        .map(numeric)
        .collect::<Option<_>>()
        .ok_or_else(bad)?;
    let core: [u64; 3] = nums.try_into().map_err(|_| bad())?;
    let mut ids = Vec::new();
    if let Some(p) = pre {
        for id in p.split('.') {
            if id.is_empty() || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
                return Err(bad());
            }
            ids.push(match numeric(id) {
                Some(n) => Ident::Num(n),
                None if id.bytes().all(|b| b.is_ascii_digit()) => return Err(bad()),
                None => Ident::Alpha(id.to_string()),
            });
        }
    }
    Ok(Version { core, pre: ids })
}

fn precedence(a: &Version, b: &Version) -> Ordering {
    a.core
        .cmp(&b.core)
        .then_with(|| match (a.pre.is_empty(), b.pre.is_empty()) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            (false, false) => {
                for (x, y) in a.pre.iter().zip(&b.pre) {
                    let o = match (x, y) {
                        (Ident::Num(m), Ident::Num(n)) => m.cmp(n),
                        (Ident::Num(_), Ident::Alpha(_)) => Ordering::Less,
                        (Ident::Alpha(_), Ident::Num(_)) => Ordering::Greater,
                        (Ident::Alpha(s), Ident::Alpha(t)) => s.cmp(t),
                    };
                    if o != Ordering::Equal {
                        return o;
                    }
                }
                a.pre.len().cmp(&b.pre.len())
            }
        })
}

/// Pure logic: a stable sort, so equal precedence keeps the input order.
fn sort(versions: &[String]) -> Result<Vec<String>, String> {
    let mut parsed: Vec<(Version, &String)> = versions
        .iter()
        .map(|v| parse(v).map(|p| (p, v)))
        .collect::<Result<_, _>>()?;
    parsed.sort_by(|a, b| precedence(&a.0, &b.0));
    Ok(parsed.into_iter().map(|(_, v)| v.clone()).collect())
}

fn run(input: &str) -> Result<String, Fail> {
    let input: Input = serde_json::from_str(input).map_err(|e| Fail::Input(e.to_string()))?;
    let sorted = sort(&input.versions).map_err(Fail::Input)?;
    Ok(json!({"sorted": sorted}).to_string())
}

enum Fail {
    Input(String),
}

#[cfg(target_arch = "wasm32")]
mod bindings {
    wit_bindgen::generate!({
        path: "../../wit/init-ext/v1",
        world: "init:ext/tool",
    });

    use super::{DESCRIPTION, Fail, NAME, SCHEMA, VERSION};

    struct Component;

    impl Guest for Component {
        fn describe() -> ToolInfo {
            ToolInfo {
                name: NAME.to_string(),
                description: DESCRIPTION.to_string(),
                input_schema: SCHEMA.to_string(),
                version: VERSION.to_string(),
                timeout_ms: None,
            }
        }

        fn call(input: String) -> Result<String, ToolError> {
            super::run(&input).map_err(|f| match f {
                Fail::Input(m) => ToolError::InvalidInput(m),
            })
        }
    }

    export!(Component);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_org_example_order() {
        let v: Vec<String> = [
            "1.0.0",
            "1.0.0-rc.1",
            "1.0.0-beta.11",
            "1.0.0-alpha",
            "1.0.0-beta",
            "1.0.0-alpha.beta",
            "1.0.0-beta.2",
            "1.0.0-alpha.1",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let want = [
            "1.0.0-alpha",
            "1.0.0-alpha.1",
            "1.0.0-alpha.beta",
            "1.0.0-beta",
            "1.0.0-beta.2",
            "1.0.0-beta.11",
            "1.0.0-rc.1",
            "1.0.0",
        ];
        assert_eq!(sort(&v).unwrap(), want);
    }

    #[test]
    fn rejects_bad_versions() {
        for b in ["1.0", "01.0.0", "1.0.0-01", "1.0.0-", "v1.0.0"] {
            assert!(parse(b).is_err(), "{b}");
        }
    }
}
