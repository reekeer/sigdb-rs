//! JSON value with the data model of Python's `json` module: object keys keep their order,
//! integers of any size stay exact and `1` is distinct from `1.0`.

use std::ops::{Deref, DerefMut};

use indexmap::IndexMap;

/// JSON object: keys in insertion order, like a Python `dict`. Derefs to an [`IndexMap`]; it is
/// boxed so that a [`Value`] stays 32 bytes.
#[derive(Clone, Default, PartialEq)]
pub struct Map(Box<IndexMap<String, Value>>);

impl Map {
    pub fn new() -> Self {
        Map::default()
    }

    pub fn with_capacity(n: usize) -> Self {
        Map(Box::new(IndexMap::with_capacity(n)))
    }

    pub fn into_inner(self) -> IndexMap<String, Value> {
        *self.0
    }
}

impl Deref for Map {
    type Target = IndexMap<String, Value>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Map {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl std::fmt::Debug for Map {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl From<IndexMap<String, Value>> for Map {
    fn from(m: IndexMap<String, Value>) -> Self {
        Map(Box::new(m))
    }
}

impl FromIterator<(String, Value)> for Map {
    fn from_iter<I: IntoIterator<Item = (String, Value)>>(iter: I) -> Self {
        Map(Box::new(iter.into_iter().collect()))
    }
}

impl Extend<(String, Value)> for Map {
    fn extend<I: IntoIterator<Item = (String, Value)>>(&mut self, iter: I) {
        self.0.extend(iter);
    }
}

impl IntoIterator for Map {
    type Item = (String, Value);
    type IntoIter = indexmap::map::IntoIter<String, Value>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a> IntoIterator for &'a Map {
    type Item = (&'a String, &'a Value);
    type IntoIter = indexmap::map::Iter<'a, String, Value>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl<'a> IntoIterator for &'a mut Map {
    type Item = (&'a String, &'a mut Value);
    type IntoIter = indexmap::map::IterMut<'a, String, Value>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter_mut()
    }
}

/// A JSON value as Python's `json.loads` produces it.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Value {
    #[default]
    Null,
    Bool(bool),
    /// An integer that fits `i64`.
    Int(i64),
    /// An integer outside the `i64` range, as canonical decimal text (`-?[1-9][0-9]*`).
    BigInt(String),
    Float(f64),
    String(String),
    Array(Vec<Value>),
    Object(Map),
}

impl Value {
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Int(i) => Some(*i as f64),
            Value::BigInt(s) => s.parse().ok(),
            Value::Float(f) => Some(*f),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&Vec<Value>> {
        match self {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&Map> {
        match self {
            Value::Object(m) => Some(m),
            _ => None,
        }
    }

    pub fn as_object_mut(&mut self) -> Option<&mut Map> {
        match self {
            Value::Object(m) => Some(m),
            _ => None,
        }
    }

    /// `value[key]` for objects, `None` for a missing key or a non-object.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.as_object().and_then(|m| m.get(key))
    }

    /// Serializes like `json.dumps(value, ensure_ascii=False, separators=(",", ":"))`.
    pub fn to_json(&self) -> crate::Result<String> {
        crate::json::dump(self).map_err(|e| crate::Error::format(e.0))
    }

    /// Parses JSON text like `json.loads`.
    pub fn from_json(text: &str) -> crate::Result<Value> {
        crate::json::parse(text).map_err(|e| crate::Error::format(format!("invalid json: {}", e.0)))
    }
}

macro_rules! from_int {
    ($($t:ty),*) => {$(
        impl From<$t> for Value {
            fn from(v: $t) -> Self {
                Value::Int(v as i64)
            }
        }
    )*};
}

from_int!(i8, i16, i32, i64, u8, u16, u32);

impl From<u64> for Value {
    fn from(v: u64) -> Self {
        match i64::try_from(v) {
            Ok(i) => Value::Int(i),
            Err(_) => Value::BigInt(v.to_string()),
        }
    }
}

impl From<usize> for Value {
    fn from(v: usize) -> Self {
        Value::from(v as u64)
    }
}

impl From<i128> for Value {
    fn from(v: i128) -> Self {
        match i64::try_from(v) {
            Ok(i) => Value::Int(i),
            Err(_) => Value::BigInt(v.to_string()),
        }
    }
}

impl From<f64> for Value {
    fn from(v: f64) -> Self {
        Value::Float(v)
    }
}

impl From<f32> for Value {
    fn from(v: f32) -> Self {
        Value::Float(v as f64)
    }
}

impl From<bool> for Value {
    fn from(v: bool) -> Self {
        Value::Bool(v)
    }
}

impl From<&str> for Value {
    fn from(v: &str) -> Self {
        Value::String(v.to_string())
    }
}

impl From<String> for Value {
    fn from(v: String) -> Self {
        Value::String(v)
    }
}

impl From<&String> for Value {
    fn from(v: &String) -> Self {
        Value::String(v.clone())
    }
}

impl From<Map> for Value {
    fn from(v: Map) -> Self {
        Value::Object(v)
    }
}

impl<T: Into<Value>> From<Vec<T>> for Value {
    fn from(v: Vec<T>) -> Self {
        Value::Array(v.into_iter().map(Into::into).collect())
    }
}

impl<T: Into<Value> + Clone> From<&[T]> for Value {
    fn from(v: &[T]) -> Self {
        Value::Array(v.iter().cloned().map(Into::into).collect())
    }
}

impl<T: Into<Value>> From<Option<T>> for Value {
    fn from(v: Option<T>) -> Self {
        v.map_or(Value::Null, Into::into)
    }
}

impl<K: Into<String>, V: Into<Value>> FromIterator<(K, V)> for Value {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        Value::Object(iter.into_iter().map(|(k, v)| (k.into(), v.into())).collect())
    }
}

impl<T: Into<Value>> FromIterator<T> for Value {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        Value::Array(iter.into_iter().map(Into::into).collect())
    }
}

#[cfg(feature = "serde")]
mod serde_impl {
    use super::{Map, Value};
    use serde::de::{MapAccess, SeqAccess, Visitor};
    use serde::ser::{SerializeMap, SerializeSeq};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::fmt;

    impl Serialize for Value {
        fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
            match self {
                Value::Null => s.serialize_unit(),
                Value::Bool(b) => s.serialize_bool(*b),
                Value::Int(i) => s.serialize_i64(*i),
                Value::BigInt(t) => match t.parse::<u64>() {
                    Ok(u) => s.serialize_u64(u),
                    Err(_) => match t.parse::<i128>() {
                        Ok(i) => s.serialize_i128(i),
                        Err(_) => s.serialize_f64(t.parse().unwrap_or(f64::NAN)),
                    },
                },
                Value::Float(f) => s.serialize_f64(*f),
                Value::String(t) => s.serialize_str(t),
                Value::Array(a) => {
                    let mut seq = s.serialize_seq(Some(a.len()))?;
                    for v in a {
                        seq.serialize_element(v)?;
                    }
                    seq.end()
                }
                Value::Object(m) => {
                    let mut map = s.serialize_map(Some(m.len()))?;
                    for (k, v) in m {
                        map.serialize_entry(k, v)?;
                    }
                    map.end()
                }
            }
        }
    }

    struct ValueVisitor;

    impl<'de> Visitor<'de> for ValueVisitor {
        type Value = Value;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("any JSON value")
        }

        fn visit_bool<E>(self, v: bool) -> Result<Value, E> {
            Ok(Value::Bool(v))
        }

        fn visit_i64<E>(self, v: i64) -> Result<Value, E> {
            Ok(Value::Int(v))
        }

        fn visit_i128<E>(self, v: i128) -> Result<Value, E> {
            Ok(Value::from(v))
        }

        fn visit_u64<E>(self, v: u64) -> Result<Value, E> {
            Ok(Value::from(v))
        }

        fn visit_u128<E>(self, v: u128) -> Result<Value, E> {
            Ok(match i64::try_from(v) {
                Ok(i) => Value::Int(i),
                Err(_) => Value::BigInt(v.to_string()),
            })
        }

        fn visit_f64<E>(self, v: f64) -> Result<Value, E> {
            Ok(Value::Float(v))
        }

        fn visit_str<E>(self, v: &str) -> Result<Value, E> {
            Ok(Value::String(v.to_string()))
        }

        fn visit_string<E>(self, v: String) -> Result<Value, E> {
            Ok(Value::String(v))
        }

        fn visit_unit<E>(self) -> Result<Value, E> {
            Ok(Value::Null)
        }

        fn visit_none<E>(self) -> Result<Value, E> {
            Ok(Value::Null)
        }

        fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Value, D::Error> {
            Deserialize::deserialize(d)
        }

        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
            let mut out = Vec::with_capacity(seq.size_hint().unwrap_or(0).min(4096));
            while let Some(v) = seq.next_element()? {
                out.push(v);
            }
            Ok(Value::Array(out))
        }

        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
            let mut out = Map::with_capacity(map.size_hint().unwrap_or(0).min(4096));
            while let Some((k, v)) = map.next_entry::<String, Value>()? {
                out.insert(k, v);
            }
            Ok(Value::Object(out))
        }
    }

    impl<'de> Deserialize<'de> for Value {
        fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Value, D::Error> {
            d.deserialize_any(ValueVisitor)
        }
    }

    impl From<serde_json::Value> for Value {
        fn from(v: serde_json::Value) -> Self {
            match v {
                serde_json::Value::Null => Value::Null,
                serde_json::Value::Bool(b) => Value::Bool(b),
                serde_json::Value::Number(n) => {
                    if let Some(i) = n.as_i64() {
                        Value::Int(i)
                    } else if let Some(u) = n.as_u64() {
                        Value::from(u)
                    } else {
                        Value::Float(n.as_f64().unwrap_or(f64::NAN))
                    }
                }
                serde_json::Value::String(s) => Value::String(s),
                serde_json::Value::Array(a) => Value::Array(a.into_iter().map(Value::from).collect()),
                serde_json::Value::Object(m) => {
                    Value::Object(m.into_iter().map(|(k, v)| (k, Value::from(v))).collect())
                }
            }
        }
    }

    impl From<Value> for serde_json::Value {
        fn from(v: Value) -> Self {
            match v {
                Value::Null => serde_json::Value::Null,
                Value::Bool(b) => serde_json::Value::Bool(b),
                Value::Int(i) => serde_json::Value::from(i),
                Value::BigInt(t) => match t.parse::<u64>() {
                    Ok(u) => serde_json::Value::from(u),
                    Err(_) => serde_json::Number::from_f64(t.parse().unwrap_or(f64::NAN))
                        .map_or(serde_json::Value::Null, serde_json::Value::Number),
                },
                Value::Float(f) => {
                    serde_json::Number::from_f64(f).map_or(serde_json::Value::Null, serde_json::Value::Number)
                }
                Value::String(s) => serde_json::Value::String(s),
                Value::Array(a) => {
                    serde_json::Value::Array(a.into_iter().map(serde_json::Value::from).collect())
                }
                Value::Object(m) => serde_json::Value::Object(
                    m.into_iter().map(|(k, v)| (k, serde_json::Value::from(v))).collect(),
                ),
            }
        }
    }
}

static NULL: Value = Value::Null;

impl std::ops::Index<&str> for Value {
    type Output = Value;

    /// `value[key]`; `Null` for a missing key or a non-object, like `serde_json`.
    fn index(&self, key: &str) -> &Value {
        self.get(key).unwrap_or(&NULL)
    }
}

impl std::ops::Index<usize> for Value {
    type Output = Value;

    /// `value[i]`; `Null` when out of range or not an array.
    fn index(&self, i: usize) -> &Value {
        self.as_array().and_then(|a| a.get(i)).unwrap_or(&NULL)
    }
}

impl std::fmt::Display for Value {
    /// The compact JSON text; non-finite floats print as Python's `repr` would.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match crate::json::dump(self) {
            Ok(s) => f.write_str(&s),
            Err(_) => write!(f, "{self:?}"),
        }
    }
}
