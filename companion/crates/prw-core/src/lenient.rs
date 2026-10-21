//! Reading what other programs wrote: the serde helpers behind the protocol types.
//!
//! The shared files are written by every pr-watch version there has been and by `prw`, and
//! their PRs are GitHub's words. So the parse is the boundary, and it is forgiving field by
//! field: a field that is missing, `null` or of the wrong JSON type reads as absent (its
//! `Default`), never as a failed parse of the whole file. Only text that isn't JSON at all,
//! or a file that isn't an object, fails (`protocol::parse_shared`).
//!
//! The defaults are chosen to say what the mod's TypeScript says of the same odd value:
//! absent reads as JS `undefined`, which its code treats as "not that state" (`=== 'OPEN'`
//! is false, `=== true` is false, `?? ''` is empty), and an array item that isn't an object
//! reads as an item with no fields, as `item.name` on a number reads `undefined`.

use std::collections::BTreeMap;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};
use serde_json::Value;

/// A field that doesn't read as `T` reads as `T::default()`.
pub(crate) fn lenient<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned + Default,
{
    let value = Value::deserialize(d)?;
    Ok(T::deserialize(value).unwrap_or_default())
}

/// A list whose items each read leniently: an item that doesn't read as `T` is
/// `T::default()`, not dropped (the TypeScript counts it, as an item with no fields). Not a
/// list: empty.
pub(crate) fn lenient_vec<'de, D, T>(d: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned + Default,
{
    Ok(vec_of(Value::deserialize(d)?).unwrap_or_default())
}

/// As [`lenient_vec`], where the list itself may be `null` (or absent): `None`.
pub(crate) fn lenient_vec_opt<'de, D, T>(d: D) -> Result<Option<Vec<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned + Default,
{
    Ok(vec_of(Value::deserialize(d)?))
}

fn vec_of<T: DeserializeOwned + Default>(value: Value) -> Option<Vec<T>> {
    match value {
        Value::Array(items) => Some(
            items
                .into_iter()
                .map(|item| T::deserialize(item).unwrap_or_default())
                .collect(),
        ),
        _ => None,
    }
}

/// Only the strings of a list, in order (`urlsIn`): anything else in it is skipped, and
/// anything but a list is empty.
pub(crate) fn strings<'de, D>(d: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(strings_of(Value::deserialize(d)?))
}

pub(crate) fn strings_of(value: Value) -> Vec<String> {
    match value {
        Value::Array(items) => items
            .into_iter()
            .filter_map(|item| match item {
                Value::String(s) => Some(s),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// A map whose values each read leniently: a value that doesn't read as `T` is dropped
/// (it was never one: a map value has no "no fields" reading). Not an object: empty.
pub(crate) fn map_of<T: DeserializeOwned>(value: Value) -> BTreeMap<String, T> {
    match value {
        Value::Object(map) => map
            .into_iter()
            .filter_map(|(key, value)| T::deserialize(value).ok().map(|t| (key, t)))
            .collect(),
        _ => BTreeMap::new(),
    }
}

/// A time in milliseconds since the epoch, or 0 where there is none.
pub(crate) fn ms<'de, D>(d: D) -> Result<i64, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(ms_of(&Value::deserialize(d)?).unwrap_or(0))
}

/// A time in milliseconds since the epoch, where there is one.
pub(crate) fn ms_opt<'de, D>(d: D) -> Result<Option<i64>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(ms_of(&Value::deserialize(d)?))
}

/// JS writes its numbers as doubles: a whole one reads exactly, a fractional one (a
/// `retry-after` of 1.5 s makes `heldUntil` one) rounds up, so a hold is never cut short.
/// Not a finite number: none.
pub(crate) fn ms_of(value: &Value) -> Option<i64> {
    let Value::Number(n) = value else { return None };
    if let Some(i) = n.as_i64() {
        return Some(i);
    }
    if n.as_u64().is_some() {
        return Some(i64::MAX);
    }
    let f = n.as_f64()?;
    #[expect(
        clippy::cast_possible_truncation,
        reason = "`as` saturates, and a time past i64 is no time anyone meant"
    )]
    let whole = f.ceil() as i64;
    f.is_finite().then_some(whole)
}

/// A string-valued enum with an escape: each known value is a variant, and any other string
/// is kept as `Other`, so a value GitHub (or a newer writer) adds never fails a parse, and
/// is written back as it was read. `Default` is `Other("")`: absent.
macro_rules! string_enum {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident {
            $( $(#[$vmeta:meta])* $variant:ident = $text:literal ),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        $vis enum $name {
            $( $(#[$vmeta])* $variant, )+
            /// A value this version doesn't know (or `""`: none was given), as it was written.
            Other(String),
        }

        impl $name {
            /// The value as it is written in the file.
            #[must_use]
            pub fn as_str(&self) -> &str {
                match self {
                    $( Self::$variant => $text, )+
                    Self::Other(text) => text,
                }
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::Other(String::new())
            }
        }

        impl From<&str> for $name {
            fn from(text: &str) -> Self {
                match text {
                    $( $text => Self::$variant, )+
                    other => Self::Other(other.to_owned()),
                }
            }
        }

        impl From<String> for $name {
            fn from(text: String) -> Self {
                match text.as_str() {
                    $( $text => Self::$variant, )+
                    _ => Self::Other(text),
                }
            }
        }

        impl ::std::fmt::Display for $name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl ::serde::Serialize for $name {
            fn serialize<S: ::serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(self.as_str())
            }
        }

        impl<'de> ::serde::Deserialize<'de> for $name {
            fn deserialize<D: ::serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                String::deserialize(d).map(Self::from)
            }
        }
    };
}

pub(crate) use string_enum;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn times_read_whole_and_fractional_numbers() {
        assert_eq!(
            ms_of(&json!(1_791_373_800_000_i64)),
            Some(1_791_373_800_000)
        );
        assert_eq!(ms_of(&json!(1.5)), Some(2));
        assert_eq!(ms_of(&json!(-1.5)), Some(-1));
        assert_eq!(ms_of(&json!(u64::MAX)), Some(i64::MAX));
        assert_eq!(ms_of(&json!("12")), None);
        assert_eq!(ms_of(&json!(null)), None);
    }

    #[test]
    fn strings_keeps_only_strings() {
        assert_eq!(
            strings_of(json!(["a", 1, null, {"b": 2}, "c"])),
            vec!["a".to_owned(), "c".to_owned()]
        );
        assert_eq!(strings_of(json!({"0": "a"})), Vec::<String>::new());
    }
}
