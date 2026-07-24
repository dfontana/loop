//! Structured ledger variables — the inputs a `when` guard reasons over.
//!
//! Vars are a JSON object of scopes: `{"build": {"status": "pass", "id": "b-8842"},
//! "qa": {"result": "fail", "error_class": "transient"}}`. They arrive either
//! from a tool's `LOOP_VARS` line (trusted — a real exit code asserted it) or
//! from the worker's `transition(vars=…)` (untrusted hints; never gate on them).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Vars(pub Map<String, Value>);

impl Vars {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Recursively merge `other` over `self`: objects merge key-wise, every
    /// other value is replaced wholesale.
    pub fn merge(&mut self, other: &Vars) {
        deep_merge(&mut self.0, &other.0);
    }

    /// Look up a dotted path, e.g. `qa.error_class`.
    pub fn get_path(&self, path: &str) -> Option<&Value> {
        let mut cur = self.0.get(path.split('.').next()?)?;
        for seg in path.split('.').skip(1) {
            cur = cur.as_object()?.get(seg)?;
        }
        Some(cur)
    }

    pub fn as_value(&self) -> Value {
        Value::Object(self.0.clone())
    }

    pub fn from_value(v: Value) -> Self {
        match v {
            Value::Object(m) => Vars(m),
            _ => Vars::default(),
        }
    }

    /// Flatten to the `$UPPER_SNAKE` names the context namespace exposes:
    /// `build.id` becomes `BUILD_ID`. Only scalars are exposed; nested objects
    /// recurse, arrays are skipped.
    pub fn to_env(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        flatten(&self.0, "", &mut out);
        out.sort();
        out
    }
}

fn flatten(map: &Map<String, Value>, prefix: &str, out: &mut Vec<(String, String)>) {
    for (k, v) in map {
        let name = if prefix.is_empty() {
            k.to_uppercase()
        } else {
            format!("{prefix}_{}", k.to_uppercase())
        };
        match v {
            Value::Object(inner) => flatten(inner, &name, out),
            Value::String(s) => out.push((name, s.clone())),
            Value::Number(n) => out.push((name, n.to_string())),
            Value::Bool(b) => out.push((name, b.to_string())),
            Value::Null | Value::Array(_) => {}
        }
    }
}

fn deep_merge(base: &mut Map<String, Value>, other: &Map<String, Value>) {
    for (k, v) in other {
        match (base.get_mut(k), v) {
            (Some(Value::Object(b)), Value::Object(o)) => deep_merge(b, o),
            _ => {
                base.insert(k.clone(), v.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn merge_is_deep_and_last_wins() {
        let mut a = Vars::from_value(json!({"qa": {"result": "fail", "detail": "boom"}}));
        a.merge(&Vars::from_value(
            json!({"qa": {"result": "pass"}, "build": {"id": "b-1"}}),
        ));
        assert_eq!(a.get_path("qa.result").unwrap(), "pass");
        assert_eq!(a.get_path("qa.detail").unwrap(), "boom");
        assert_eq!(a.get_path("build.id").unwrap(), "b-1");
    }

    #[test]
    fn env_flattening_uppercases_and_joins() {
        let v = Vars::from_value(json!({"build": {"id": "b-8842", "attempts": 2}}));
        assert_eq!(
            v.to_env(),
            vec![
                ("BUILD_ATTEMPTS".to_string(), "2".to_string()),
                ("BUILD_ID".to_string(), "b-8842".to_string()),
            ]
        );
    }
}
