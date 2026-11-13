//! A port of beets 2.14's `types` plugin (`beetsplug/types.py`), and the
//! plugins' flexible attribute types (beets' `item_types` /
//! `album_types`, `plugins.types()`): the types change how a flexible
//! attribute is parsed (`modify`), stored, formatted, queried (numeric and
//! date ranges, booleans) and sorted.

use crate::config::Config;
use crate::model::FieldType;
use anyhow::{Result, bail};
use serde_yaml::Value as Y;
use std::collections::BTreeMap;

/// The plugin: only types.
pub struct TypesPlugin;

impl crate::plugins::Plugin for TypesPlugin {
    fn name(&self) -> &'static str {
        "types"
    }
}

/// `TypesPlugin._types`: the `types:` settings, `field: int|float|bool|date`,
/// in their order.
pub fn config_types(cfg: &Config) -> Result<Vec<(String, FieldType)>> {
    let Some(Y::Mapping(m)) = cfg.get("types") else {
        return Ok(vec![]);
    };
    let mut out = vec![];
    for (k, v) in m {
        let key = scalar(k);
        let ty = match v {
            Y::String(s) if s == "int" => FieldType::Int,
            Y::String(s) if s == "float" => FieldType::Float,
            Y::String(s) if s == "bool" => FieldType::Bool,
            Y::String(s) if s == "date" => FieldType::Date,
            other => bail!(
                "configuration error: unknown type '{}' for the '{key}' field",
                scalar(other)
            ),
        };
        out.push((key, ty));
    }
    Ok(out)
}

fn scalar(v: &Y) -> String {
    match v {
        Y::String(s) => s.clone(),
        other => serde_yaml::to_string(other)
            .unwrap_or_default()
            .trim()
            .to_string(),
    }
}

/// The flexible attribute types a plugin declares, for items and albums.
fn declared(cfg: &Config, plugin: &str) -> Result<[Vec<(String, FieldType)>; 2]> {
    let own = |v: &[(&str, FieldType)]| -> Vec<(String, FieldType)> {
        v.iter().map(|(k, t)| (k.to_string(), *t)).collect()
    };
    Ok(match plugin {
        "types" => {
            let t = config_types(cfg)?;
            [t.clone(), t]
        }
        "lyrics" => [own(crate::plugins::lyrics::ITEM_TYPES), vec![]],
        "missing" => [vec![], own(crate::plugins::missing::ALBUM_TYPES)],
        _ => [vec![], vec![]],
    })
}

/// Collect the enabled plugins' types, in plugin order (beets'
/// `plugins.types`): a field declared again with another type is an error.
pub fn flex_types(cfg: &Config) -> Result<[BTreeMap<String, FieldType>; 2]> {
    let mut out: [BTreeMap<String, FieldType>; 2] = Default::default();
    for name in crate::plugins::names(cfg) {
        let decl = declared(cfg, &name)?;
        for (i, types) in decl.into_iter().enumerate() {
            for (field, ty) in types {
                if out[i].get(&field).is_some_and(|t| *t != ty) {
                    bail!(
                        "Plugin {name} defines flexible field {field} which has already \
                         been defined with another type."
                    );
                }
                out[i].insert(field, ty);
            }
        }
    }
    Ok(out)
}

/// Declare the enabled plugins' types for this run.
pub fn configure(cfg: &Config) -> Result<()> {
    let [items, albums] = flex_types(cfg)?;
    crate::model::set_flex_types(items, albums);
    Ok(())
}

#[cfg(test)]
#[path = "types_tests.rs"]
mod tests;

// ---------------------------------------------------------------- registry

inventory::submit! {
    crate::plugin_registry::PluginDef {
        name: "types",
        order: 25,
        defaults: r#""#,
        config_keys: &["types"],
        redacted: &[],
        make: Some(registry_make),
        fatal: false,
        commands: &[],
    }
}

fn registry_make(_cfg: &crate::config::Config) -> anyhow::Result<Box<dyn crate::plugins::Plugin>> {
    Ok(Box::new(TypesPlugin))
}
