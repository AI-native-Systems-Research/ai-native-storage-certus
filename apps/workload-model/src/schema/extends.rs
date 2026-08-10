//! `extends`: deep-merge a preset beneath a document.
//!
//! The including document wins on every conflicting leaf, and **lists replace
//! rather than append**. Replacement is the less obvious choice and the right
//! one: a mixture is a whole, so appending to a base's `mix` would silently
//! produce a four-entry mixture from two two-entry documents, and the weights
//! would no longer mean what either author wrote.
//!
//! The merge runs on the untyped value tree *before* typed deserialisation,
//! because it is a structural operation and because a partial preset need not be
//! a valid document on its own — only the merged result must be.
//!
//! ```
//! use workload_model::schema::extends::merge;
//! let base: serde_yaml::Value = serde_yaml::from_str("a: 1\nb: {c: 2, d: 3}").unwrap();
//! let over: serde_yaml::Value = serde_yaml::from_str("b: {c: 9}").unwrap();
//! let m = merge(base, over);
//! assert_eq!(m["a"], serde_yaml::Value::from(1));   // inherited
//! assert_eq!(m["b"]["c"], serde_yaml::Value::from(9)); // overridden
//! assert_eq!(m["b"]["d"], serde_yaml::Value::from(3)); // sibling kept
//! ```

use serde_yaml::{Mapping, Value};

/// Deep-merge `over` onto `base`, with `over` winning every conflict.
pub fn merge(base: Value, over: Value) -> Value {
    match (base, over) {
        (Value::Mapping(b), Value::Mapping(o)) => {
            let mut out: Mapping = b;
            for (k, ov) in o {
                let merged = match out.remove(&k) {
                    Some(bv) => merge(bv, ov),
                    None => ov,
                };
                out.insert(k, merged);
            }
            Value::Mapping(out)
        }
        // Lists replace. See the module note: appending would change what a
        // mixture's weights mean.
        (_, over) => over,
    }
}

/// How far `extends` chains may be followed before declaring a cycle.
pub const MAX_DEPTH: usize = 16;

/// Something wrong with an `extends` chain.
#[derive(Debug, PartialEq, Eq)]
pub enum ExtendsError {
    /// The chain revisited a path, or exceeded [`MAX_DEPTH`].
    Cycle(String),
    /// A preset could not be read.
    Unreadable { path: String, reason: String },
    /// A preset was not valid YAML.
    Malformed { path: String, reason: String },
}

impl std::fmt::Display for ExtendsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExtendsError::Cycle(p) => write!(
                f,
                "extends chain cycles or exceeds {MAX_DEPTH} levels at `{p}`"
            ),
            ExtendsError::Unreadable { path, reason } => {
                write!(f, "cannot read preset `{path}`: {reason}")
            }
            ExtendsError::Malformed { path, reason } => {
                write!(f, "preset `{path}` is not valid YAML: {reason}")
            }
        }
    }
}

impl std::error::Error for ExtendsError {}

/// Resolve an `extends` chain, returning the fully merged value tree.
///
/// `read` maps a preset path to its contents, so callers choose whether that
/// means the filesystem or something else — which is what lets this be tested
/// without touching disk.
pub fn resolve<F>(root: Value, read: &F) -> Result<Value, ExtendsError>
where
    F: Fn(&str) -> Result<String, String>,
{
    resolve_inner(root, read, &mut Vec::new())
}

fn resolve_inner<F>(mut doc: Value, read: &F, seen: &mut Vec<String>) -> Result<Value, ExtendsError>
where
    F: Fn(&str) -> Result<String, String>,
{
    let path = match doc.get("extends").and_then(|v| v.as_str()) {
        Some(p) => p.to_string(),
        None => return Ok(doc),
    };

    if seen.contains(&path) || seen.len() >= MAX_DEPTH {
        return Err(ExtendsError::Cycle(path));
    }
    seen.push(path.clone());

    let text = read(&path).map_err(|reason| ExtendsError::Unreadable {
        path: path.clone(),
        reason,
    })?;
    let base: Value = serde_yaml::from_str(&text).map_err(|e| ExtendsError::Malformed {
        path: path.clone(),
        reason: e.to_string(),
    })?;

    // The including document's own `extends` key is consumed rather than
    // inherited, so the merged result records no chain it no longer has.
    if let Value::Mapping(m) = &mut doc {
        m.remove(Value::from("extends"));
    }

    let base = resolve_inner(base, read, seen)?;
    Ok(merge(base, doc))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn y(s: &str) -> Value {
        serde_yaml::from_str(s).unwrap()
    }

    #[test]
    fn including_document_wins_on_every_leaf() {
        let m = merge(y("a: 1\nb: 2"), y("b: 9"));
        assert_eq!(m["a"], Value::from(1));
        assert_eq!(m["b"], Value::from(9));
    }

    #[test]
    fn nested_maps_merge_rather_than_replace() {
        let m = merge(y("s: {x: 1, y: 2}"), y("s: {y: 9}"));
        assert_eq!(m["s"]["x"], Value::from(1), "sibling must survive");
        assert_eq!(m["s"]["y"], Value::from(9));
    }

    #[test]
    fn lists_replace_and_do_not_append() {
        // A mixture is a whole. Appending would turn two two-entry documents
        // into a four-entry mixture whose weights mean neither author's intent.
        let m = merge(
            y("mix: [{weight: 1}, {weight: 2}]"),
            y("mix: [{weight: 5}]"),
        );
        assert_eq!(m["mix"].as_sequence().unwrap().len(), 1);
        assert_eq!(m["mix"][0]["weight"], Value::from(5));
    }

    #[test]
    fn a_chain_is_followed_and_the_nearest_document_wins() {
        let read = |p: &str| -> Result<String, String> {
            Ok(match p {
                "mid.yaml" => "extends: base.yaml\nb: 2\nc: 2".to_string(),
                "base.yaml" => "a: 1\nb: 1\nc: 1".to_string(),
                other => return Err(format!("no such preset {other}")),
            })
        };
        let top = y("extends: mid.yaml\nc: 3");
        let m = resolve(top, &read).unwrap();
        assert_eq!(m["a"], Value::from(1), "from base");
        assert_eq!(m["b"], Value::from(2), "mid beats base");
        assert_eq!(m["c"], Value::from(3), "top beats both");
    }

    #[test]
    fn the_extends_key_is_consumed_not_inherited() {
        let read = |_: &str| Ok("a: 1".to_string());
        let m = resolve(y("extends: base.yaml"), &read).unwrap();
        assert!(
            m.get("extends").is_none(),
            "merged result kept a stale chain"
        );
    }

    #[test]
    fn a_cycle_is_reported_rather_than_looping() {
        let read = |p: &str| -> Result<String, String> {
            Ok(match p {
                "a.yaml" => "extends: b.yaml".to_string(),
                "b.yaml" => "extends: a.yaml".to_string(),
                _ => return Err("nope".into()),
            })
        };
        let e = resolve(y("extends: a.yaml"), &read).unwrap_err();
        assert!(matches!(e, ExtendsError::Cycle(_)), "got {e:?}");
    }

    #[test]
    fn a_missing_preset_names_itself() {
        let read = |_: &str| Err("no such file".to_string());
        let e = resolve(y("extends: gone.yaml"), &read).unwrap_err();
        match e {
            ExtendsError::Unreadable { path, .. } => assert_eq!(path, "gone.yaml"),
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn a_malformed_preset_names_itself() {
        let read = |_: &str| Ok("a: [unclosed".to_string());
        let e = resolve(y("extends: bad.yaml"), &read).unwrap_err();
        assert!(matches!(e, ExtendsError::Malformed { .. }), "got {e:?}");
    }

    #[test]
    fn a_document_without_extends_is_returned_unchanged() {
        let read = |_: &str| -> Result<String, String> { panic!("must not read") };
        let m = resolve(y("a: 1"), &read).unwrap();
        assert_eq!(m["a"], Value::from(1));
    }
}
