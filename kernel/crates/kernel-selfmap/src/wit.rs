//! WIT exports and imports of a compiled component (selfmap.md §3.6 step 1; ext/host.md §3.15).
//!
//! Static: the binary is decoded with wit-component 0.262.0, nothing is executed. Only exports, imports
//! and `wit_sha256` are kept; the names of the synthetic root package and world that `decode` produces
//! are not stored.

use anyhow::{Result, bail};
use indexmap::IndexMap;
use serde_json::{Value, json};
use wit_component::{DecodedWasm, WitPrinter};
use wit_parser::{Function, Resolve, Type, TypeDefKind, WorldItem, WorldKey};

use crate::icj::sha256_tag;

/// One exported or imported item (selfmap.md §4.2 `wit_item`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WitItem {
    Func {
        name: String,
        signature: String,
    },
    Interface {
        name: String,
        functions: Vec<String>,
    },
    Type {
        name: String,
    },
}

impl WitItem {
    pub fn name(&self) -> &str {
        match self {
            WitItem::Func { name, .. }
            | WitItem::Interface { name, .. }
            | WitItem::Type { name } => name,
        }
    }

    pub fn to_value(&self) -> Value {
        match self {
            WitItem::Func { name, signature } => {
                json!({"kind": "func", "name": name, "signature": signature})
            }
            WitItem::Interface { name, functions } => {
                json!({"functions": functions, "kind": "interface", "name": name})
            }
            WitItem::Type { name } => json!({"kind": "type", "name": name}),
        }
    }
}

/// The WIT inventory of one component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WitInventory {
    pub exports: Vec<WitItem>,
    pub imports: Vec<WitItem>,
    /// `"sha256:" + hex(SHA-256(WitPrinter text of the decoded package))`.
    pub wit_sha256: String,
}

/// Prints a WIT type: primitives by WIT name, named types by name, anonymous ones structurally.
pub fn type_text(resolve: &Resolve, t: &Type) -> String {
    match t {
        Type::Bool => "bool".into(),
        Type::U8 => "u8".into(),
        Type::U16 => "u16".into(),
        Type::U32 => "u32".into(),
        Type::U64 => "u64".into(),
        Type::S8 => "s8".into(),
        Type::S16 => "s16".into(),
        Type::S32 => "s32".into(),
        Type::S64 => "s64".into(),
        Type::F32 => "f32".into(),
        Type::F64 => "f64".into(),
        Type::Char => "char".into(),
        Type::String => "string".into(),
        Type::ErrorContext => "error-context".into(),
        Type::Id(id) => {
            let def = &resolve.types[*id];
            if let Some(n) = &def.name {
                return n.clone();
            }
            let opt = |o: &Option<Type>| match o {
                Some(t) => type_text(resolve, t),
                None => "_".into(),
            };
            match &def.kind {
                TypeDefKind::List(t) => format!("list<{}>", type_text(resolve, t)),
                TypeDefKind::FixedLengthList(t, n) => {
                    format!("list<{}, {n}>", type_text(resolve, t))
                }
                TypeDefKind::Map(k, v) => {
                    format!("map<{}, {}>", type_text(resolve, k), type_text(resolve, v))
                }
                TypeDefKind::Option(t) => format!("option<{}>", type_text(resolve, t)),
                TypeDefKind::Result(r) => match (&r.ok, &r.err) {
                    (None, None) => "result".into(),
                    (Some(o), None) => format!("result<{}>", type_text(resolve, o)),
                    (o, e) => format!("result<{}, {}>", opt(o), opt(e)),
                },
                TypeDefKind::Tuple(t) => format!(
                    "tuple<{}>",
                    t.types
                        .iter()
                        .map(|x| type_text(resolve, x))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                TypeDefKind::Handle(wit_parser::Handle::Own(r)) => {
                    type_text(resolve, &Type::Id(*r))
                }
                TypeDefKind::Handle(wit_parser::Handle::Borrow(r)) => {
                    format!("borrow<{}>", type_text(resolve, &Type::Id(*r)))
                }
                TypeDefKind::Future(t) => format!("future<{}>", opt(t)),
                TypeDefKind::Stream(t) => format!("stream<{}>", opt(t)),
                TypeDefKind::Type(t) => type_text(resolve, t),
                _ => "_".into(),
            }
        }
    }
}

/// `func(<name>: <type>, …) -> <type>`.
pub fn func_text(resolve: &Resolve, f: &Function) -> String {
    let params = f
        .params
        .iter()
        .map(|p| format!("{}: {}", p.name, type_text(resolve, &p.ty)))
        .collect::<Vec<_>>()
        .join(", ");
    match &f.result {
        Some(t) => format!("func({params}) -> {}", type_text(resolve, t)),
        None => format!("func({params})"),
    }
}

fn render(resolve: &Resolve, items: &IndexMap<WorldKey, WorldItem>) -> Vec<WitItem> {
    let mut v: Vec<WitItem> = items
        .iter()
        .map(|(k, it)| match it {
            WorldItem::Function(f) => WitItem::Func {
                name: resolve.name_world_key(k),
                signature: func_text(resolve, f),
            },
            WorldItem::Interface { id, .. } => {
                let mut functions: Vec<String> =
                    resolve.interfaces[*id].functions.keys().cloned().collect();
                functions.sort();
                WitItem::Interface {
                    name: resolve
                        .id_of(*id)
                        .unwrap_or_else(|| resolve.name_world_key(k)),
                    functions,
                }
            }
            WorldItem::Type { .. } => WitItem::Type {
                name: resolve.name_world_key(k),
            },
        })
        .collect();
    v.sort_by(|a, b| a.name().cmp(b.name()));
    v
}

/// Decodes a component and lists its world's exports and imports, sorted by name.
pub fn wit_inventory(bytes: &[u8]) -> Result<WitInventory> {
    let decoded = wit_component::decode(bytes)?;
    let (resolve, world_id) = match &decoded {
        DecodedWasm::Component(r, w) => (r, *w),
        DecodedWasm::WitPackage(..) => bail!("not a component (a WIT package)"),
    };
    let world = &resolve.worlds[world_id];
    let mut printer = WitPrinter::default();
    printer.print(resolve, decoded.package(), &[])?;
    let wit_text: String = printer.output.to_string();
    Ok(WitInventory {
        exports: render(resolve, &world.exports),
        imports: render(resolve, &world.imports),
        wit_sha256: sha256_tag(wit_text.as_bytes()),
    })
}
