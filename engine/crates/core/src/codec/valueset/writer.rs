//! The `pbvalueset` writer.

use super::{Point, Rect, Size, Value, ValueSet, field};
use uuid::Uuid;

pub(super) const WT_VARINT: u8 = 0;
pub(super) const WT_FIXED64: u8 = 1;
pub(super) const WT_LEN: u8 = 2;
pub(super) const WT_FIXED32: u8 = 5;

#[derive(Default)]
pub(super) struct W(Vec<u8>);

impl W {
    fn varint(&mut self, mut v: u64) {
        while v >= 0x80 {
            self.0.push((v as u8) | 0x80);
            v >>= 7;
        }
        self.0.push(v as u8);
    }
    fn key(&mut self, field: u32, wt: u8) {
        self.varint(u64::from(field << 3 | u32::from(wt)));
    }
    fn len_delimited(&mut self, field: u32, body: &[u8]) {
        self.key(field, WT_LEN);
        self.varint(body.len() as u64);
        self.0.extend_from_slice(body);
    }
    /// One length-delimited field per item, in order (a repeated field).
    fn len_delimited_each<B: AsRef<[u8]>>(&mut self, field: u32, items: impl Iterator<Item = B>) {
        for body in items {
            self.len_delimited(field, body.as_ref());
        }
    }
    fn varint_field(&mut self, field: u32, v: u64) {
        if v != 0 {
            self.key(field, WT_VARINT);
            self.varint(v);
        }
    }
    fn packed(&mut self, field: u32, values: impl Iterator<Item = u64>) {
        let mut body = W::default();
        for v in values {
            body.varint(v);
        }
        if !body.0.is_empty() {
            self.len_delimited(field, &body.0);
        }
    }
    fn packed_fixed32(&mut self, field: u32, values: &[f32]) {
        if values.is_empty() {
            return;
        }
        let body: Vec<u8> = values.iter().flat_map(|f| f.to_bits().to_le_bytes()).collect();
        self.len_delimited(field, &body);
    }
    fn packed_fixed64(&mut self, field: u32, values: &[f64]) {
        if values.is_empty() {
            return;
        }
        let body: Vec<u8> = values.iter().flat_map(|f| f.to_bits().to_le_bytes()).collect();
        self.len_delimited(field, &body);
    }
}

/// int32 fields carry negative values as the sign-extended 64-bit varint (10 bytes).
pub(super) fn i32_bits(v: i32) -> u64 {
    i64::from(v) as u64
}

pub(super) fn float_field(w: &mut W, field: u32, v: f32) {
    if v != 0.0 {
        w.key(field, WT_FIXED32);
        w.0.extend_from_slice(&v.to_bits().to_le_bytes());
    }
}

pub(super) fn point_body(p: &Point) -> Vec<u8> {
    let mut w = W::default();
    float_field(&mut w, 1, p.x);
    float_field(&mut w, 2, p.y);
    w.0
}
pub(super) fn size_body(s: &Size) -> Vec<u8> {
    let mut w = W::default();
    float_field(&mut w, 1, s.width);
    float_field(&mut w, 2, s.height);
    w.0
}
pub(super) fn rect_body(r: &Rect) -> Vec<u8> {
    let mut w = W::default();
    float_field(&mut w, 1, r.x);
    float_field(&mut w, 2, r.y);
    float_field(&mut w, 3, r.width);
    float_field(&mut w, 4, r.height);
    w.0
}

pub(super) fn guid_string(g: &Uuid) -> String {
    g.hyphenated().to_string()
}

pub(super) fn encode_variant(v: &Value) -> Vec<u8> {
    let mut w = W::default();
    w.varint_field(field::VALUE_TYPE, u64::from(v.value_type()));
    match v {
        Value::UInt8(x) => w.varint_field(field::UINT8, u64::from(*x)),
        Value::Int16(x) => w.varint_field(field::INT16, i32_bits(i32::from(*x))),
        Value::UInt16(x) => w.varint_field(field::UINT16, u64::from(*x)),
        Value::Int32(x) => w.varint_field(field::INT32, i32_bits(*x)),
        Value::UInt32(x) => w.varint_field(field::UINT32, u64::from(*x)),
        Value::Int64(x) => w.varint_field(field::INT64, *x as u64),
        Value::UInt64(x) => w.varint_field(field::UINT64, *x),
        Value::Single(x) => float_field(&mut w, field::SINGLE, *x),
        Value::Double(x) => {
            if *x != 0.0 {
                w.key(field::DOUBLE, WT_FIXED64);
                w.0.extend_from_slice(&x.to_bits().to_le_bytes());
            }
        }
        Value::Char16(x) => w.varint_field(field::CHAR16, u64::from(*x)),
        Value::Bool(x) => w.varint_field(field::BOOL, u64::from(*x)),
        Value::String(s) => {
            if !s.is_empty() {
                w.len_delimited(field::STRING, s.as_bytes());
            }
        }
        Value::Guid(g) => w.len_delimited(field::GUID, guid_string(g).as_bytes()),
        Value::Point(p) => w.len_delimited(field::POINT, &point_body(p)),
        Value::Size(s) => w.len_delimited(field::SIZE, &size_body(s)),
        Value::Rect(r) => w.len_delimited(field::RECT, &rect_body(r)),
        Value::Object(o) => w.len_delimited(field::OBJECT, &encode(o)),
        Value::DateTime(x) => w.varint_field(field::DATETIME, *x as u64),
        Value::TimeSpan(x) => w.varint_field(field::TIMESPAN, *x as u64),
        Value::UInt8Array(b) => {
            if !b.is_empty() {
                w.len_delimited(field::UINT8_ARRAY, b);
            }
        }
        Value::Int16Array(a) => w.packed(field::INT16_ARRAY, a.iter().map(|x| i32_bits(i32::from(*x)))),
        Value::UInt16Array(a) => w.packed(field::UINT16_ARRAY, a.iter().map(|x| u64::from(*x))),
        Value::Int32Array(a) => w.packed(field::INT32_ARRAY, a.iter().map(|x| i32_bits(*x))),
        Value::UInt32Array(a) => w.packed(field::UINT32_ARRAY, a.iter().map(|x| u64::from(*x))),
        Value::Int64Array(a) => w.packed(field::INT64_ARRAY, a.iter().map(|x| *x as u64)),
        Value::UInt64Array(a) => w.packed(field::UINT64_ARRAY, a.iter().copied()),
        Value::SingleArray(a) => w.packed_fixed32(field::SINGLE_ARRAY, a),
        Value::DoubleArray(a) => w.packed_fixed64(field::DOUBLE_ARRAY, a),
        Value::Char16Array(a) => w.packed(field::CHAR16_ARRAY, a.iter().map(|x| u64::from(*x))),
        Value::BoolArray(a) => w.packed(field::BOOL_ARRAY, a.iter().map(|x| u64::from(*x))),
        Value::StringArray(a) => w.len_delimited_each(field::STRING_ARRAY, a.iter().map(String::as_bytes)),
        Value::GuidArray(a) => w.len_delimited_each(field::GUID_ARRAY, a.iter().map(guid_string)),
        Value::PointArray(a) => w.len_delimited_each(field::POINT_ARRAY, a.iter().map(point_body)),
        Value::SizeArray(a) => w.len_delimited_each(field::SIZE_ARRAY, a.iter().map(size_body)),
        Value::RectArray(a) => w.len_delimited_each(field::RECT_ARRAY, a.iter().map(rect_body)),
        Value::ObjectArray(a) => w.len_delimited_each(field::OBJECT_ARRAY, a.iter().map(encode)),
        Value::DateTimeArray(a) => w.packed(field::DATETIME_ARRAY, a.iter().map(|x| *x as u64)),
        Value::TimeSpanArray(a) => w.packed(field::TIMESPAN_ARRAY, a.iter().map(|x| *x as u64)),
        Value::Empty => {}
    }

    w.0
}

/// Serializes a `ValueSet` (no length prefix; its length is the envelope's `payloadLength`).
pub fn encode(vs: &ValueSet) -> Vec<u8> {
    let mut w = W::default();
    for (k, v) in &vs.entries {
        let mut entry = W::default();
        if !k.is_empty() {
            entry.len_delimited(1, k.as_bytes());
        }
        entry.len_delimited(2, &encode_variant(v));
        w.len_delimited(1, &entry.0);
    }
    w.0
}
