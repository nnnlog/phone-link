//! The `pbvalueset` `ValueSet` — the payload of every App message (schema
//! `crates/proto/proto/value_set/valueset.proto`).
//!
//! The codec is hand-written so the map keeps the writer's entry order (the wire form is identical
//! to `map<string, DataVariant>`) and the proto3 default rules are explicit:
//! - every entry writes `value_type`; the value field only when it differs from the proto3 default
//!   (`0`, `false`, `""`, empty bytes, empty list); message-typed values (`Point`, `Size`, `Rect`,
//!   `Object`) are always written; a map entry's key is written only when non-empty;
//! - repeated scalars are written packed and read packed or unpacked; strings, GUIDs and messages
//!   are one element per tag;
//! - a reader takes the field's default when `value_type` names an absent field (an absent GUID has
//!   no default form and fails); a singular field arriving with an unexpected wire type is skipped
//!   like any unknown field; an unknown or `Invalid` `value_type` fails the whole message.

use serde_json::json;
use uuid::Uuid;

mod reader;
mod writer;

pub use reader::{MAX_DEPTH, ValueSetError, decode, gunzip};
pub use writer::encode;
use writer::{WT_FIXED32, WT_FIXED64, WT_LEN, WT_VARINT};

/// `Point` — two floats.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

/// `Size` — a width and a height as two floats.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Size {
    pub width: f32,
    pub height: f32,
}

/// `Rect` — an origin, a width and a height as four floats.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// One `DataVariant` value. `DateTime` / `TimeSpan` are the raw `WinRT` ABI `int64` values
/// (100-ns intervals since 1601-01-01 UTC, and 100-ns duration units); helpers below convert.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    UInt8(u8),
    Int16(i16),
    UInt16(u16),
    Int32(i32),
    UInt32(u32),
    Int64(i64),
    UInt64(u64),
    Single(f32),
    Double(f64),
    /// One UTF-16 code unit.
    Char16(u16),
    Bool(bool),
    String(String),
    Guid(Uuid),
    Point(Point),
    Size(Size),
    Rect(Rect),
    Object(ValueSet),
    DateTime(i64),
    TimeSpan(i64),
    UInt8Array(Vec<u8>),
    Int16Array(Vec<i16>),
    UInt16Array(Vec<u16>),
    Int32Array(Vec<i32>),
    UInt32Array(Vec<u32>),
    Int64Array(Vec<i64>),
    UInt64Array(Vec<u64>),
    SingleArray(Vec<f32>),
    DoubleArray(Vec<f64>),
    Char16Array(Vec<u16>),
    BoolArray(Vec<bool>),
    StringArray(Vec<String>),
    GuidArray(Vec<Uuid>),
    PointArray(Vec<Point>),
    SizeArray(Vec<Size>),
    RectArray(Vec<Rect>),
    ObjectArray(Vec<ValueSet>),
    DateTimeArray(Vec<i64>),
    TimeSpanArray(Vec<i64>),
    /// A `null` entry value (`value_type = 35`, no value field).
    Empty,
}

/// `DataValueType` codes (all 40 values).
pub mod value_type {
    /// `0`: no valid type; a message carrying it fails to decode.
    pub const INVALID: u32 = 0;
    /// `1`: an unsigned 8-bit integer.
    pub const UINT8: u32 = 1;
    /// `2`: a signed 16-bit integer.
    pub const INT16: u32 = 2;
    /// `3`: an unsigned 16-bit integer.
    pub const UINT16: u32 = 3;
    /// `4`: a signed 32-bit integer.
    pub const INT32: u32 = 4;
    /// `5`: an unsigned 32-bit integer.
    pub const UINT32: u32 = 5;
    /// `6`: a signed 64-bit integer.
    pub const INT64: u32 = 6;
    /// `7`: an unsigned 64-bit integer.
    pub const UINT64: u32 = 7;
    /// `8`: a 32-bit float.
    pub const SINGLE: u32 = 8;
    /// `9`: a 64-bit float.
    pub const DOUBLE: u32 = 9;
    /// `10`: one UTF-16 code unit.
    pub const CHAR16: u32 = 10;
    /// `11`: a boolean.
    pub const BOOL: u32 = 11;
    /// `12`: a UTF-8 string.
    pub const STRING: u32 = 12;
    /// `13`: a GUID.
    pub const GUID: u32 = 13;
    /// `14`: a [`Point`].
    pub const POINT: u32 = 14;
    /// `15`: a [`Size`].
    pub const SIZE: u32 = 15;
    /// `16`: a [`Rect`].
    pub const RECT: u32 = 16;
    /// `17`: a nested [`ValueSet`].
    pub const OBJECT: u32 = 17;
    /// `18`: an array of unsigned 8-bit integers.
    pub const UINT8_ARRAY: u32 = 18;
    /// `19`: an array of signed 16-bit integers.
    pub const INT16_ARRAY: u32 = 19;
    /// `20`: an array of unsigned 16-bit integers.
    pub const UINT16_ARRAY: u32 = 20;
    /// `21`: an array of signed 32-bit integers.
    pub const INT32_ARRAY: u32 = 21;
    /// `22`: an array of unsigned 32-bit integers.
    pub const UINT32_ARRAY: u32 = 22;
    /// `23`: an array of signed 64-bit integers.
    pub const INT64_ARRAY: u32 = 23;
    /// `24`: an array of unsigned 64-bit integers.
    pub const UINT64_ARRAY: u32 = 24;
    /// `25`: an array of 32-bit floats.
    pub const SINGLE_ARRAY: u32 = 25;
    /// `26`: an array of 64-bit floats.
    pub const DOUBLE_ARRAY: u32 = 26;
    /// `27`: an array of UTF-16 code units.
    pub const CHAR16_ARRAY: u32 = 27;
    /// `28`: an array of booleans.
    pub const BOOL_ARRAY: u32 = 28;
    /// `29`: an array of strings.
    pub const STRING_ARRAY: u32 = 29;
    /// `30`: an array of GUIDs.
    pub const GUID_ARRAY: u32 = 30;
    /// `31`: an array of [`Point`]s.
    pub const POINT_ARRAY: u32 = 31;
    /// `32`: an array of [`Size`]s.
    pub const SIZE_ARRAY: u32 = 32;
    /// `33`: an array of [`Rect`]s.
    pub const RECT_ARRAY: u32 = 33;
    /// `34`: an array of nested [`ValueSet`]s.
    pub const OBJECT_ARRAY: u32 = 34;
    /// `35`: a `null` entry with no value field.
    pub const EMPTY: u32 = 35;
    /// `36`: a date and time, 100-ns intervals since 1601-01-01 UTC.
    pub const DATETIME: u32 = 36;
    /// `37`: a duration in 100-ns units.
    pub const TIMESPAN: u32 = 37;
    /// `38`: an array of date-and-time values.
    pub const DATETIME_ARRAY: u32 = 38;
    /// `39`: an array of durations.
    pub const TIMESPAN_ARRAY: u32 = 39;
}

// DataVariant field numbers (they differ from the value_type codes).
mod field {
    pub(super) const VALUE_TYPE: u32 = 1;
    pub(super) const UINT8: u32 = 2;
    pub(super) const INT16: u32 = 3;
    pub(super) const UINT16: u32 = 4;
    pub(super) const INT32: u32 = 5;
    pub(super) const UINT32: u32 = 6;
    pub(super) const INT64: u32 = 7;
    pub(super) const UINT64: u32 = 8;
    pub(super) const SINGLE: u32 = 9;
    pub(super) const DOUBLE: u32 = 10;
    pub(super) const CHAR16: u32 = 11;
    pub(super) const BOOL: u32 = 12;
    pub(super) const STRING: u32 = 13;
    pub(super) const GUID: u32 = 14;
    pub(super) const POINT: u32 = 15;
    pub(super) const SIZE: u32 = 16;
    pub(super) const RECT: u32 = 17;
    pub(super) const OBJECT: u32 = 18;
    pub(super) const DATETIME: u32 = 19;
    pub(super) const TIMESPAN: u32 = 20;
    pub(super) const UINT8_ARRAY: u32 = 21;
    pub(super) const INT16_ARRAY: u32 = 22;
    pub(super) const UINT16_ARRAY: u32 = 23;
    pub(super) const INT32_ARRAY: u32 = 24;
    pub(super) const UINT32_ARRAY: u32 = 25;
    pub(super) const INT64_ARRAY: u32 = 26;
    pub(super) const UINT64_ARRAY: u32 = 27;
    pub(super) const SINGLE_ARRAY: u32 = 28;
    pub(super) const DOUBLE_ARRAY: u32 = 29;
    pub(super) const CHAR16_ARRAY: u32 = 30;
    pub(super) const BOOL_ARRAY: u32 = 31;
    pub(super) const STRING_ARRAY: u32 = 32;
    pub(super) const GUID_ARRAY: u32 = 33;
    pub(super) const POINT_ARRAY: u32 = 34;
    pub(super) const SIZE_ARRAY: u32 = 35;
    pub(super) const RECT_ARRAY: u32 = 36;
    pub(super) const OBJECT_ARRAY: u32 = 37;
    pub(super) const DATETIME_ARRAY: u32 = 38;
    pub(super) const TIMESPAN_ARRAY: u32 = 39;
}

impl Value {
    /// The `DataValueType` code written for this value.
    pub fn value_type(&self) -> u32 {
        use value_type as t;
        match self {
            Value::UInt8(_) => t::UINT8,
            Value::Int16(_) => t::INT16,
            Value::UInt16(_) => t::UINT16,
            Value::Int32(_) => t::INT32,
            Value::UInt32(_) => t::UINT32,
            Value::Int64(_) => t::INT64,
            Value::UInt64(_) => t::UINT64,
            Value::Single(_) => t::SINGLE,
            Value::Double(_) => t::DOUBLE,
            Value::Char16(_) => t::CHAR16,
            Value::Bool(_) => t::BOOL,
            Value::String(_) => t::STRING,
            Value::Guid(_) => t::GUID,
            Value::Point(_) => t::POINT,
            Value::Size(_) => t::SIZE,
            Value::Rect(_) => t::RECT,
            Value::Object(_) => t::OBJECT,
            Value::DateTime(_) => t::DATETIME,
            Value::TimeSpan(_) => t::TIMESPAN,
            Value::UInt8Array(_) => t::UINT8_ARRAY,
            Value::Int16Array(_) => t::INT16_ARRAY,
            Value::UInt16Array(_) => t::UINT16_ARRAY,
            Value::Int32Array(_) => t::INT32_ARRAY,
            Value::UInt32Array(_) => t::UINT32_ARRAY,
            Value::Int64Array(_) => t::INT64_ARRAY,
            Value::UInt64Array(_) => t::UINT64_ARRAY,
            Value::SingleArray(_) => t::SINGLE_ARRAY,
            Value::DoubleArray(_) => t::DOUBLE_ARRAY,
            Value::Char16Array(_) => t::CHAR16_ARRAY,
            Value::BoolArray(_) => t::BOOL_ARRAY,
            Value::StringArray(_) => t::STRING_ARRAY,
            Value::GuidArray(_) => t::GUID_ARRAY,
            Value::PointArray(_) => t::POINT_ARRAY,
            Value::SizeArray(_) => t::SIZE_ARRAY,
            Value::RectArray(_) => t::RECT_ARRAY,
            Value::ObjectArray(_) => t::OBJECT_ARRAY,
            Value::DateTimeArray(_) => t::DATETIME_ARRAY,
            Value::TimeSpanArray(_) => t::TIMESPAN_ARRAY,
            Value::Empty => t::EMPTY,
        }
    }

    /// The `DataValueType` name without its prefix (`String`, `Int32Array`, …).
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::UInt8(_) => "UInt8",
            Value::Int16(_) => "Int16",
            Value::UInt16(_) => "UInt16",
            Value::Int32(_) => "Int32",
            Value::UInt32(_) => "UInt32",
            Value::Int64(_) => "Int64",
            Value::UInt64(_) => "UInt64",
            Value::Single(_) => "Single",
            Value::Double(_) => "Double",
            Value::Char16(_) => "Char16",
            Value::Bool(_) => "Bool",
            Value::String(_) => "String",
            Value::Guid(_) => "Guid",
            Value::Point(_) => "Point",
            Value::Size(_) => "Size",
            Value::Rect(_) => "Rect",
            Value::Object(_) => "Object",
            Value::DateTime(_) => "DateTime",
            Value::TimeSpan(_) => "TimeSpan",
            Value::UInt8Array(_) => "UInt8Array",
            Value::Int16Array(_) => "Int16Array",
            Value::UInt16Array(_) => "UInt16Array",
            Value::Int32Array(_) => "Int32Array",
            Value::UInt32Array(_) => "UInt32Array",
            Value::Int64Array(_) => "Int64Array",
            Value::UInt64Array(_) => "UInt64Array",
            Value::SingleArray(_) => "SingleArray",
            Value::DoubleArray(_) => "DoubleArray",
            Value::Char16Array(_) => "Char16Array",
            Value::BoolArray(_) => "BoolArray",
            Value::StringArray(_) => "StringArray",
            Value::GuidArray(_) => "GuidArray",
            Value::PointArray(_) => "PointArray",
            Value::SizeArray(_) => "SizeArray",
            Value::RectArray(_) => "RectArray",
            Value::ObjectArray(_) => "ObjectArray",
            Value::DateTimeArray(_) => "DateTimeArray",
            Value::TimeSpanArray(_) => "TimeSpanArray",
            Value::Empty => "Empty",
        }
    }
}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::String(s.to_string())
    }
}
impl From<String> for Value {
    fn from(s: String) -> Self {
        Value::String(s)
    }
}
impl From<i32> for Value {
    fn from(v: i32) -> Self {
        Value::Int32(v)
    }
}
impl From<u32> for Value {
    fn from(v: u32) -> Self {
        Value::UInt32(v)
    }
}
impl From<i64> for Value {
    fn from(v: i64) -> Self {
        Value::Int64(v)
    }
}
impl From<bool> for Value {
    fn from(v: bool) -> Self {
        Value::Bool(v)
    }
}
impl From<f64> for Value {
    fn from(v: f64) -> Self {
        Value::Double(v)
    }
}
impl From<ValueSet> for Value {
    fn from(v: ValueSet) -> Self {
        Value::Object(v)
    }
}
impl From<Vec<u8>> for Value {
    fn from(v: Vec<u8>) -> Self {
        Value::UInt8Array(v)
    }
}
impl From<Vec<i32>> for Value {
    fn from(v: Vec<i32>) -> Self {
        Value::Int32Array(v)
    }
}
impl From<Vec<u32>> for Value {
    fn from(v: Vec<u32>) -> Self {
        Value::UInt32Array(v)
    }
}
impl From<Vec<i64>> for Value {
    fn from(v: Vec<i64>) -> Self {
        Value::Int64Array(v)
    }
}
impl From<Vec<bool>> for Value {
    fn from(v: Vec<bool>) -> Self {
        Value::BoolArray(v)
    }
}
impl From<Vec<String>> for Value {
    fn from(v: Vec<String>) -> Self {
        Value::StringArray(v)
    }
}
impl From<Vec<&str>> for Value {
    fn from(v: Vec<&str>) -> Self {
        Value::StringArray(v.into_iter().map(str::to_string).collect())
    }
}

/// An ordered `string → Value` map with unique keys (the `entries` map of `ValueSet`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ValueSet {
    entries: Vec<(String, Value)>,
}

impl ValueSet {
    /// An empty map.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets `key`, keeping its position when present.
    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<Value>) {
        let key = key.into();
        let value = value.into();
        match self.entries.iter_mut().find(|(k, _)| *k == key) {
            Some(slot) => slot.1 = value,
            None => self.entries.push((key, value)),
        }
    }

    /// Builder form of [`ValueSet::insert`].
    pub fn with(mut self, key: impl Into<String>, value: impl Into<Value>) -> Self {
        self.insert(key, value);
        self
    }

    /// The value of `key`, or `None` when absent.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// Removes `key` and returns its value; later entries keep their order.
    pub fn remove(&mut self, key: &str) -> Option<Value> {
        let idx = self.entries.iter().position(|(k, _)| k == key)?;
        Some(self.entries.remove(idx).1)
    }

    /// Whether `key` is present.
    pub fn contains_key(&self, key: &str) -> bool {
        self.entries.iter().any(|(k, _)| k == key)
    }

    /// The entries in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// The keys in insertion order.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|(k, _)| k.as_str())
    }

    /// The number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the map has no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    // Typed reads: `Some` only when the key holds exactly that type.

    /// The value of `key` if it has type `T`; absent or of another type ⇒ `None`.
    pub fn read<'a, T: FromValue<'a>>(&'a self, key: &str) -> Option<T> {
        self.get(key).and_then(T::from_value)
    }

    /// The value of `key` if it has type `T`; absent ⇒ `Ok(None)`; another type ⇒ [`Mistyped`].
    pub fn read_checked<'a, T: FromValue<'a>>(&'a self, key: &str) -> Result<Option<T>, Mistyped> {
        match self.get(key) {
            None => Ok(None),
            Some(v) => T::from_value(v).map(Some).ok_or(Mistyped { found: v.type_name(), want: T::TYPE_NAME }),
        }
    }

    /// A `String`.
    pub fn str(&self, key: &str) -> Option<&str> {
        self.read(key)
    }
    /// An `Int32`.
    pub fn i32(&self, key: &str) -> Option<i32> {
        self.read(key)
    }
    /// A `UInt32`.
    pub fn u32(&self, key: &str) -> Option<u32> {
        self.read(key)
    }
    /// An `Int64`.
    pub fn i64(&self, key: &str) -> Option<i64> {
        self.read(key)
    }
    /// A `Double`.
    pub fn f64(&self, key: &str) -> Option<f64> {
        self.read(key)
    }
    /// A `Bool`.
    pub fn bool(&self, key: &str) -> Option<bool> {
        self.read(key)
    }
    /// An `Object`.
    pub fn object(&self, key: &str) -> Option<&ValueSet> {
        self.read(key)
    }
    /// An `Int32Array`.
    pub fn i32_array(&self, key: &str) -> Option<&[i32]> {
        self.read(key)
    }
    /// A `StringArray`.
    pub fn string_array(&self, key: &str) -> Option<&[String]> {
        self.read(key)
    }
    /// An `Int64Array`.
    pub fn i64_array(&self, key: &str) -> Option<&[i64]> {
        self.read(key)
    }
    /// A `UInt32Array`.
    pub fn u32_array(&self, key: &str) -> Option<&[u32]> {
        self.read(key)
    }
    /// A `BoolArray`.
    pub fn bool_array(&self, key: &str) -> Option<&[bool]> {
        self.read(key)
    }
    /// A `UInt8Array` (byte blob).
    pub fn bytes(&self, key: &str) -> Option<&[u8]> {
        self.read(key)
    }

    /// A typed JSON view for the tap and for goldens: `{ "<key>": { "<Type>": <value> }, … }`.
    pub fn to_json(&self) -> serde_json::Value {
        let mut obj = serde_json::Map::new();
        for (k, v) in &self.entries {
            obj.insert(k.clone(), value_to_json(v));
        }
        serde_json::Value::Object(obj)
    }
}

/// A key of `found` type where `want` was expected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("a {found} where a {want} was expected")]
pub struct Mistyped {
    pub found: &'static str,
    pub want: &'static str,
}

/// A Rust type one `Value` variant holds, read by [`ValueSet::read`].
pub trait FromValue<'a>: Sized {
    /// The variant's name, as [`Value::type_name`] gives it.
    const TYPE_NAME: &'static str;
    /// The held value, if `value` is that variant.
    fn from_value(value: &'a Value) -> Option<Self>;
}

macro_rules! from_value {
    ($t:ty, $variant:ident, copy) => {
        impl FromValue<'_> for $t {
            const TYPE_NAME: &'static str = stringify!($variant);
            fn from_value(value: &Value) -> Option<Self> {
                if let Value::$variant(v) = value { Some(*v) } else { None }
            }
        }
    };
    ($t:ty, $variant:ident, borrow) => {
        impl<'a> FromValue<'a> for &'a $t {
            const TYPE_NAME: &'static str = stringify!($variant);
            fn from_value(value: &'a Value) -> Option<Self> {
                if let Value::$variant(v) = value { Some(v) } else { None }
            }
        }
    };
}

from_value!(i32, Int32, copy);
from_value!(u32, UInt32, copy);
from_value!(i64, Int64, copy);
from_value!(f64, Double, copy);
from_value!(bool, Bool, copy);
from_value!(str, String, borrow);
from_value!(ValueSet, Object, borrow);
from_value!([i32], Int32Array, borrow);
from_value!([u32], UInt32Array, borrow);
from_value!([i64], Int64Array, borrow);
from_value!([bool], BoolArray, borrow);
from_value!([u8], UInt8Array, borrow);
from_value!([String], StringArray, borrow);

fn point_json(p: &Point) -> serde_json::Value {
    json!({ "X": p.x, "Y": p.y })
}
fn size_json(s: &Size) -> serde_json::Value {
    json!({ "Width": s.width, "Height": s.height })
}
fn rect_json(r: &Rect) -> serde_json::Value {
    json!({ "X": r.x, "Y": r.y, "Width": r.width, "Height": r.height })
}

/// `{ "<Type>": <value> }` for one value.
pub fn value_to_json(v: &Value) -> serde_json::Value {
    let inner = match v {
        Value::UInt8(x) => json!(x),
        Value::Int16(x) => json!(x),
        Value::UInt16(x) => json!(x),
        Value::Int32(x) => json!(x),
        Value::UInt32(x) => json!(x),
        Value::Int64(x) => json!(x),
        Value::UInt64(x) => json!(x),
        Value::Single(x) => json!(x),
        Value::Double(x) => json!(x),
        Value::Char16(x) => json!(x),
        Value::Bool(x) => json!(x),
        Value::String(x) => json!(x),
        Value::Guid(x) => json!(x.hyphenated().to_string()),
        Value::Point(p) => point_json(p),
        Value::Size(s) => size_json(s),
        Value::Rect(r) => rect_json(r),
        Value::Object(o) => o.to_json(),
        Value::DateTime(x) | Value::TimeSpan(x) => json!(x),
        Value::UInt8Array(x) => json!(hex::encode(x)),
        Value::Int16Array(x) => json!(x),
        Value::UInt16Array(x) => json!(x),
        Value::Int32Array(x) => json!(x),
        Value::UInt32Array(x) => json!(x),
        Value::Int64Array(x) => json!(x),
        Value::UInt64Array(x) => json!(x),
        Value::SingleArray(x) => json!(x),
        Value::DoubleArray(x) => json!(x),
        Value::Char16Array(x) => json!(x),
        Value::BoolArray(x) => json!(x),
        Value::StringArray(x) => json!(x),
        Value::GuidArray(x) => json!(x.iter().map(|g| g.hyphenated().to_string()).collect::<Vec<_>>()),
        Value::PointArray(x) => serde_json::Value::Array(x.iter().map(point_json).collect()),
        Value::SizeArray(x) => serde_json::Value::Array(x.iter().map(size_json).collect()),
        Value::RectArray(x) => serde_json::Value::Array(x.iter().map(rect_json).collect()),
        Value::ObjectArray(x) => serde_json::Value::Array(x.iter().map(ValueSet::to_json).collect()),
        Value::DateTimeArray(x) | Value::TimeSpanArray(x) => json!(x),
        Value::Empty => serde_json::Value::Null,
    };

    json!({ v.type_name(): inner })
}

#[cfg(test)]
mod tests;
