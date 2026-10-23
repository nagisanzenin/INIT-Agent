//! The `self_v` 2 JSON Schemas (selfmap.md §4.2 and §4.2a), verbatim from the doc.
//!
//! `SELF_V2` describes both the committed file (no `live`, D12) and the served view (with `live`).
//! The integration tests validate every generated file against them (`tests/common/mod.rs` holds a small
//! draft 2020-12 validator for the keywords these two schemas use).

/// `https://init.local/schemas/self-v2.json`.
pub const SELF_V2: &str = include_str!("../schema/self-v2.json");
/// `https://init.local/schemas/kernel-self-v2.json`.
pub const KERNEL_SELF_V2: &str = include_str!("../schema/kernel-self-v2.json");

#[cfg(test)]
mod tests {
    use serde_json::Value;

    #[test]
    fn schemas_parse() {
        let s: Value = serde_json::from_str(super::SELF_V2).unwrap();
        assert_eq!(s["properties"]["self_v"]["const"], 2);
        assert!(
            s["required"]
                .as_array()
                .unwrap()
                .iter()
                .all(|r| r != "live"),
            "live is optional (D12)"
        );
        let k: Value = serde_json::from_str(super::KERNEL_SELF_V2).unwrap();
        assert_eq!(
            k["properties"]["kernel"]["$ref"],
            "self-v2.json#/$defs/kernel_tree"
        );
    }
}
