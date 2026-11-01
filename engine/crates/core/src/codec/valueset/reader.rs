//! The `pbvalueset` reader.

use super::{Point, Rect, Size, Value, ValueSet, WT_FIXED32, WT_FIXED64, WT_LEN, WT_VARINT, field, value_type};
use std::io::Read;
use uuid::Uuid;

/// Why a payload is not a `ValueSet`.
///
/// The engine discards the message, answers `1` when its headers
/// carry `_requestId` and `_route = /legacy/phonecontent` (Phone Link on Windows does not answer
/// it), and otherwise only logs it (the fragments are already acked).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ValueSetError {
    #[error("protobuf truncated")]
    Truncated,
    #[error("varint longer than 10 bytes")]
    VarintOverflow,
    #[error("unsupported wire type {0}")]
    WireType(u8),
    #[error("field number 0")]
    FieldZero,
    #[error("unknown value_type {0}")]
    UnknownValueType(u64),
    #[error("value_type Invalid (0)")]
    InvalidValueType,
    #[error("map entry without a value")]
    MissingValue,
    #[error("string is not valid UTF-8")]
    Utf8,
    #[error("invalid GUID {0:?}")]
    Guid(String),
    #[error("nesting deeper than {0} levels")]
    TooDeep(usize),
    #[error("gzip payload: {0}")]
    Gzip(String),
}

/// The nesting limit.
pub const MAX_DEPTH: usize = 100;

#[derive(Clone, Copy)]
pub(super) enum Raw<'a> {
    Varint(u64),
    Fixed64(u64),
    Len(&'a [u8]),
    Fixed32(u32),
}

pub(super) struct R<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> R<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }
    fn done(&self) -> bool {
        self.pos >= self.buf.len()
    }
    fn varint(&mut self) -> Result<u64, ValueSetError> {
        let mut v: u64 = 0;
        for i in 0..10 {
            let b = *self.buf.get(self.pos).ok_or(ValueSetError::Truncated)?;
            self.pos += 1;
            v |= u64::from(b & 0x7F) << (7 * i);
            if b & 0x80 == 0 {
                return Ok(v);
            }
        }
        Err(ValueSetError::VarintOverflow)
    }
    fn bytes(&mut self, n: usize) -> Result<&'a [u8], ValueSetError> {
        let end = self.pos.checked_add(n).ok_or(ValueSetError::Truncated)?;
        let s = self.buf.get(self.pos..end).ok_or(ValueSetError::Truncated)?;
        self.pos = end;
        Ok(s)
    }
    /// One field: (number, value). Groups (wire types 3/4) are not supported.
    fn field(&mut self) -> Result<(u32, Raw<'a>), ValueSetError> {
        let key = self.varint()?;
        let number = (key >> 3) as u32;
        if number == 0 {
            return Err(ValueSetError::FieldZero);
        }

        let raw = match (key & 7) as u8 {
            WT_VARINT => Raw::Varint(self.varint()?),
            WT_FIXED64 => Raw::Fixed64(u64::from_le_bytes(self.bytes(8)?.try_into().expect("8"))),
            WT_LEN => {
                let n = usize::try_from(self.varint()?).map_err(|_| ValueSetError::Truncated)?;
                Raw::Len(self.bytes(n)?)
            }
            WT_FIXED32 => Raw::Fixed32(u32::from_le_bytes(self.bytes(4)?.try_into().expect("4"))),
            other => return Err(ValueSetError::WireType(other)),
        };
        Ok((number, raw))
    }
}

pub(super) fn utf8(b: &[u8]) -> Result<String, ValueSetError> {
    std::str::from_utf8(b).map(str::to_string).map_err(|_| ValueSetError::Utf8)
}

pub(super) fn parse_guid(s: &str) -> Result<Uuid, ValueSetError> {
    Uuid::parse_str(s).map_err(|_| ValueSetError::Guid(s.to_string()))
}

/// Floats of a `Point`/`Size`/`Rect` body by field number (last occurrence wins).
pub(super) fn float_fields(body: &[u8], n: usize) -> Result<Vec<f32>, ValueSetError> {
    let mut out = vec![0f32; n];
    let mut r = R::new(body);
    while !r.done() {
        let (num, raw) = r.field()?;
        if let Raw::Fixed32(bits) = raw {
            if (1..=n as u32).contains(&num) {
                out[num as usize - 1] = f32::from_bits(bits);
            }
        }
    }
    Ok(out)
}

pub(super) fn point(body: &[u8]) -> Result<Point, ValueSetError> {
    let f = float_fields(body, 2)?;
    Ok(Point { x: f[0], y: f[1] })
}
pub(super) fn size(body: &[u8]) -> Result<Size, ValueSetError> {
    let f = float_fields(body, 2)?;
    Ok(Size { width: f[0], height: f[1] })
}
pub(super) fn rect(body: &[u8]) -> Result<Rect, ValueSetError> {
    let f = float_fields(body, 4)?;
    Ok(Rect { x: f[0], y: f[1], width: f[2], height: f[3] })
}

/// All occurrences of each field 2..=39 of one `DataVariant`.
pub(super) struct Variant<'a> {
    value_type: u64,
    fields: Vec<Vec<Raw<'a>>>,
}

impl<'a> Variant<'a> {
    fn parse(body: &'a [u8]) -> Result<Self, ValueSetError> {
        let mut v = Variant { value_type: 0, fields: vec![Vec::new(); 40] };
        let mut r = R::new(body);
        while !r.done() {
            let (num, raw) = r.field()?;
            if num == field::VALUE_TYPE {
                if let Raw::Varint(t) = raw {
                    v.value_type = t;
                }
            } else if (num as usize) < v.fields.len() {
                v.fields[num as usize].push(raw);
            }
        }
        Ok(v)
    }

    fn last_varint(&self, f: u32) -> u64 {
        self.fields[f as usize]
            .iter()
            .rev()
            .find_map(|r| match r {
                Raw::Varint(x) => Some(*x),
                _ => None,
            })
            .unwrap_or(0)
    }
    fn last_fixed32(&self, f: u32) -> u32 {
        self.fields[f as usize]
            .iter()
            .rev()
            .find_map(|r| match r {
                Raw::Fixed32(x) => Some(*x),
                _ => None,
            })
            .unwrap_or(0)
    }
    fn last_fixed64(&self, f: u32) -> u64 {
        self.fields[f as usize]
            .iter()
            .rev()
            .find_map(|r| match r {
                Raw::Fixed64(x) => Some(*x),
                _ => None,
            })
            .unwrap_or(0)
    }
    fn last_len(&self, f: u32) -> Option<&'a [u8]> {
        self.fields[f as usize].iter().rev().find_map(|r| match r {
            Raw::Len(b) => Some(*b),
            _ => None,
        })
    }
    fn all_len(&self, f: u32) -> impl Iterator<Item = &'a [u8]> + '_ {
        self.fields[f as usize].iter().filter_map(|r| match r {
            Raw::Len(b) => Some(*b),
            _ => None,
        })
    }
    /// A repeated varint field, packed or unpacked.
    fn varints(&self, f: u32) -> Result<Vec<u64>, ValueSetError> {
        let mut out = Vec::new();
        for raw in &self.fields[f as usize] {
            match raw {
                Raw::Varint(x) => out.push(*x),
                Raw::Len(body) => {
                    let mut r = R::new(body);
                    while !r.done() {
                        out.push(r.varint()?);
                    }
                }
                _ => {}
            }
        }
        Ok(out)
    }
    fn fixed32s(&self, f: u32) -> Result<Vec<u32>, ValueSetError> {
        let mut out = Vec::new();
        for raw in &self.fields[f as usize] {
            match raw {
                Raw::Fixed32(x) => out.push(*x),
                Raw::Len(body) => {
                    if body.len() % 4 != 0 {
                        return Err(ValueSetError::Truncated);
                    }
                    out.extend(body.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().expect("4"))));
                }
                _ => {}
            }
        }
        Ok(out)
    }
    fn fixed64s(&self, f: u32) -> Result<Vec<u64>, ValueSetError> {
        let mut out = Vec::new();
        for raw in &self.fields[f as usize] {
            match raw {
                Raw::Fixed64(x) => out.push(*x),
                Raw::Len(body) => {
                    if body.len() % 8 != 0 {
                        return Err(ValueSetError::Truncated);
                    }
                    out.extend(body.chunks_exact(8).map(|c| u64::from_le_bytes(c.try_into().expect("8"))));
                }
                _ => {}
            }
        }
        Ok(out)
    }

    fn value(&self, depth: usize) -> Result<Value, ValueSetError> {
        use value_type as t;
        let vt = u32::try_from(self.value_type).map_err(|_| ValueSetError::UnknownValueType(self.value_type))?;
        match vt {
            t::INVALID => Err(ValueSetError::InvalidValueType),
            t::EMPTY => Ok(Value::Empty),
            _ => {
                if let Some(v) = self.scalar_value(vt, depth)? {
                    return Ok(v);
                }
                self.array_value(vt, depth)?.ok_or(ValueSetError::UnknownValueType(self.value_type))
            }
        }
    }

    /// The value of a scalar or object type `vt`; `None` for any other type.
    fn scalar_value(&self, vt: u32, depth: usize) -> Result<Option<Value>, ValueSetError> {
        use value_type as t;
        Ok(Some(match vt {
            t::UINT8 => Value::UInt8(self.last_varint(field::UINT8) as u8),
            t::INT16 => Value::Int16(self.last_varint(field::INT16) as i32 as i16),
            t::UINT16 => Value::UInt16(self.last_varint(field::UINT16) as i32 as u16),
            t::INT32 => Value::Int32(self.last_varint(field::INT32) as i32),
            t::UINT32 => Value::UInt32(self.last_varint(field::UINT32) as u32),
            t::INT64 => Value::Int64(self.last_varint(field::INT64) as i64),
            t::UINT64 => Value::UInt64(self.last_varint(field::UINT64)),
            t::SINGLE => Value::Single(f32::from_bits(self.last_fixed32(field::SINGLE))),
            t::DOUBLE => Value::Double(f64::from_bits(self.last_fixed64(field::DOUBLE))),
            t::CHAR16 => Value::Char16(self.last_varint(field::CHAR16) as u32 as u16),
            t::BOOL => Value::Bool(self.last_varint(field::BOOL) != 0),
            t::STRING => Value::String(match self.last_len(field::STRING) {
                Some(b) => utf8(b)?,
                None => String::new(),
            }),
            t::GUID => {
                let s = match self.last_len(field::GUID) {
                    Some(b) => utf8(b)?,
                    None => String::new(),
                };
                Value::Guid(parse_guid(&s)?)
            }
            t::POINT => Value::Point(self.last_len(field::POINT).map(point).transpose()?.unwrap_or_default()),
            t::SIZE => Value::Size(self.last_len(field::SIZE).map(size).transpose()?.unwrap_or_default()),
            t::RECT => Value::Rect(self.last_len(field::RECT).map(rect).transpose()?.unwrap_or_default()),
            t::OBJECT => Value::Object(match self.last_len(field::OBJECT) {
                Some(b) => decode_at(b, depth + 1)?,
                None => ValueSet::new(),
            }),
            t::DATETIME => Value::DateTime(self.last_varint(field::DATETIME) as i64),
            t::TIMESPAN => Value::TimeSpan(self.last_varint(field::TIMESPAN) as i64),
            _ => return Ok(None),
        }))
    }

    /// The value of an array type `vt`; `None` for any other type.
    fn array_value(&self, vt: u32, depth: usize) -> Result<Option<Value>, ValueSetError> {
        use value_type as t;
        Ok(Some(match vt {
            t::UINT8_ARRAY => {
                Value::UInt8Array(self.last_len(field::UINT8_ARRAY).map(<[u8]>::to_vec).unwrap_or_default())
            }
            t::INT16_ARRAY => {
                Value::Int16Array(self.varints(field::INT16_ARRAY)?.into_iter().map(|x| x as i32 as i16).collect())
            }
            t::UINT16_ARRAY => {
                Value::UInt16Array(self.varints(field::UINT16_ARRAY)?.into_iter().map(|x| x as i32 as u16).collect())
            }
            t::INT32_ARRAY => {
                Value::Int32Array(self.varints(field::INT32_ARRAY)?.into_iter().map(|x| x as i32).collect())
            }
            t::UINT32_ARRAY => {
                Value::UInt32Array(self.varints(field::UINT32_ARRAY)?.into_iter().map(|x| x as u32).collect())
            }
            t::INT64_ARRAY => {
                Value::Int64Array(self.varints(field::INT64_ARRAY)?.into_iter().map(|x| x as i64).collect())
            }
            t::UINT64_ARRAY => Value::UInt64Array(self.varints(field::UINT64_ARRAY)?),
            t::SINGLE_ARRAY => {
                Value::SingleArray(self.fixed32s(field::SINGLE_ARRAY)?.into_iter().map(f32::from_bits).collect())
            }
            t::DOUBLE_ARRAY => {
                Value::DoubleArray(self.fixed64s(field::DOUBLE_ARRAY)?.into_iter().map(f64::from_bits).collect())
            }
            t::CHAR16_ARRAY => {
                Value::Char16Array(self.varints(field::CHAR16_ARRAY)?.into_iter().map(|x| x as u32 as u16).collect())
            }
            t::BOOL_ARRAY => Value::BoolArray(self.varints(field::BOOL_ARRAY)?.into_iter().map(|x| x != 0).collect()),
            t::STRING_ARRAY => {
                Value::StringArray(self.all_len(field::STRING_ARRAY).map(utf8).collect::<Result<_, _>>()?)
            }
            t::GUID_ARRAY => Value::GuidArray(
                self.all_len(field::GUID_ARRAY)
                    .map(|b| utf8(b).and_then(|s| parse_guid(&s)))
                    .collect::<Result<_, _>>()?,
            ),
            t::POINT_ARRAY => Value::PointArray(self.all_len(field::POINT_ARRAY).map(point).collect::<Result<_, _>>()?),
            t::SIZE_ARRAY => Value::SizeArray(self.all_len(field::SIZE_ARRAY).map(size).collect::<Result<_, _>>()?),
            t::RECT_ARRAY => Value::RectArray(self.all_len(field::RECT_ARRAY).map(rect).collect::<Result<_, _>>()?),
            t::OBJECT_ARRAY => Value::ObjectArray(
                self.all_len(field::OBJECT_ARRAY).map(|b| decode_at(b, depth + 1)).collect::<Result<_, _>>()?,
            ),
            t::DATETIME_ARRAY => {
                Value::DateTimeArray(self.varints(field::DATETIME_ARRAY)?.into_iter().map(|x| x as i64).collect())
            }
            t::TIMESPAN_ARRAY => {
                Value::TimeSpanArray(self.varints(field::TIMESPAN_ARRAY)?.into_iter().map(|x| x as i64).collect())
            }
            _ => return Ok(None),
        }))
    }
}

pub(super) fn decode_at(bytes: &[u8], depth: usize) -> Result<ValueSet, ValueSetError> {
    if depth > MAX_DEPTH {
        return Err(ValueSetError::TooDeep(MAX_DEPTH));
    }

    let mut vs = ValueSet::new();
    let mut r = R::new(bytes);
    while !r.done() {
        let (num, raw) = r.field()?;
        let (1, Raw::Len(entry)) = (num, raw) else {
            continue; // unknown field of ValueSet
        };

        let mut key = String::new();
        let mut value: Option<&[u8]> = None;
        let mut er = R::new(entry);
        while !er.done() {
            match er.field()? {
                (1, Raw::Len(k)) => key = utf8(k)?,
                (2, Raw::Len(v)) => value = Some(v),
                _ => {}
            }
        }

        let body = value.ok_or(ValueSetError::MissingValue)?;
        let v = Variant::parse(body)?.value(depth)?;
        vs.insert(key, v); // a repeated key: last wins (protobuf map semantics)
    }
    Ok(vs)
}

/// Parses a payload as one `ValueSet`.
pub fn decode(bytes: &[u8]) -> Result<ValueSet, ValueSetError> {
    decode_at(bytes, 0)
}

/// Inflates a payload sent with `_isGzippedCompressed: "true"`.
pub fn gunzip(bytes: &[u8]) -> Result<Vec<u8>, ValueSetError> {
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(bytes).read_to_end(&mut out).map_err(|e| ValueSetError::Gzip(e.to_string()))?;
    Ok(out)
}
