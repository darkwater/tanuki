//! A Serde serializer producing [`Value`].
use super::PayloadError;
use crate::{FiniteF64, Value};
use serde::ser::{self, Serialize};
use std::{cell::Cell, collections::BTreeMap};

/// Serializes into a [`Value`]. `u8_seen` is set only for the direct element
/// of a sequence, so the sequence can tell `Vec<u8>` from other lists.
pub(super) struct ValueSerializer<'a> {
    u8_seen: Option<&'a Cell<bool>>,
}

impl ValueSerializer<'static> {
    pub(super) const fn root() -> Self {
        Self { u8_seen: None }
    }
}

fn convert<T: Serialize + ?Sized>(value: &T) -> Result<Value, PayloadError> {
    value.serialize(ValueSerializer::root())
}

impl<'a> ser::Serializer for ValueSerializer<'a> {
    type Ok = Value;
    type Error = PayloadError;
    type SerializeSeq = SeqSerializer;
    type SerializeTuple = ListSerializer;
    type SerializeTupleStruct = ListSerializer;
    type SerializeTupleVariant = VariantSerializer<ListSerializer>;
    type SerializeMap = MapSerializer;
    type SerializeStruct = MapSerializer;
    type SerializeStructVariant = VariantSerializer<MapSerializer>;

    fn is_human_readable(&self) -> bool {
        false
    }

    fn serialize_bool(self, v: bool) -> Result<Value, PayloadError> {
        Ok(Value::Bool(v))
    }
    fn serialize_i8(self, v: i8) -> Result<Value, PayloadError> {
        Ok(Value::Integer(v.into()))
    }
    fn serialize_i16(self, v: i16) -> Result<Value, PayloadError> {
        Ok(Value::Integer(v.into()))
    }
    fn serialize_i32(self, v: i32) -> Result<Value, PayloadError> {
        Ok(Value::Integer(v.into()))
    }
    fn serialize_i64(self, v: i64) -> Result<Value, PayloadError> {
        Ok(Value::Integer(v))
    }
    fn serialize_i128(self, v: i128) -> Result<Value, PayloadError> {
        i64::try_from(v)
            .map(Value::Integer)
            .map_err(|_| PayloadError::IntegerOverflow)
    }
    fn serialize_u8(self, v: u8) -> Result<Value, PayloadError> {
        if let Some(seen) = self.u8_seen {
            seen.set(true);
        }
        Ok(Value::Integer(v.into()))
    }
    fn serialize_u16(self, v: u16) -> Result<Value, PayloadError> {
        Ok(Value::Integer(v.into()))
    }
    fn serialize_u32(self, v: u32) -> Result<Value, PayloadError> {
        Ok(Value::Integer(v.into()))
    }
    fn serialize_u64(self, v: u64) -> Result<Value, PayloadError> {
        i64::try_from(v)
            .map(Value::Integer)
            .map_err(|_| PayloadError::IntegerOverflow)
    }
    fn serialize_u128(self, v: u128) -> Result<Value, PayloadError> {
        i64::try_from(v)
            .map(Value::Integer)
            .map_err(|_| PayloadError::IntegerOverflow)
    }
    fn serialize_f32(self, v: f32) -> Result<Value, PayloadError> {
        self.serialize_f64(v.into())
    }
    fn serialize_f64(self, v: f64) -> Result<Value, PayloadError> {
        Ok(Value::Float(FiniteF64::new(v)?))
    }
    fn serialize_char(self, v: char) -> Result<Value, PayloadError> {
        Ok(Value::String(v.to_string()))
    }
    fn serialize_str(self, v: &str) -> Result<Value, PayloadError> {
        Ok(Value::String(v.to_owned()))
    }
    fn serialize_bytes(self, v: &[u8]) -> Result<Value, PayloadError> {
        Ok(Value::Bytes(v.to_vec()))
    }
    fn serialize_none(self) -> Result<Value, PayloadError> {
        Ok(Value::Null)
    }
    // Wrappers are transparent but never count as a direct `u8`, so neither
    // `Vec<Option<u8>>` nor a newtype list becomes bytes.
    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<Value, PayloadError> {
        convert(value)
    }
    fn serialize_unit(self) -> Result<Value, PayloadError> {
        Ok(Value::Null)
    }
    fn serialize_unit_struct(self, _name: &'static str) -> Result<Value, PayloadError> {
        Ok(Value::Null)
    }
    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
    ) -> Result<Value, PayloadError> {
        Ok(Value::String(variant.to_owned()))
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        value: &T,
    ) -> Result<Value, PayloadError> {
        convert(value)
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<Value, PayloadError> {
        Ok(Value::Map(BTreeMap::from([(
            variant.to_owned(),
            convert(value)?,
        )])))
    }
    fn serialize_seq(self, len: Option<usize>) -> Result<SeqSerializer, PayloadError> {
        Ok(SeqSerializer {
            items: Vec::with_capacity(len.unwrap_or(0)),
            all_u8: true,
        })
    }
    fn serialize_tuple(self, len: usize) -> Result<ListSerializer, PayloadError> {
        Ok(ListSerializer(Vec::with_capacity(len)))
    }
    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        len: usize,
    ) -> Result<ListSerializer, PayloadError> {
        self.serialize_tuple(len)
    }
    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<VariantSerializer<ListSerializer>, PayloadError> {
        Ok(VariantSerializer {
            variant,
            inner: ListSerializer(Vec::with_capacity(len)),
        })
    }
    fn serialize_map(self, _len: Option<usize>) -> Result<MapSerializer, PayloadError> {
        Ok(MapSerializer::default())
    }
    fn serialize_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<MapSerializer, PayloadError> {
        Ok(MapSerializer::default())
    }
    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        _len: usize,
    ) -> Result<VariantSerializer<MapSerializer>, PayloadError> {
        Ok(VariantSerializer {
            variant,
            inner: MapSerializer::default(),
        })
    }
}

/// A variable-length sequence: bytes when every element was a `u8`.
pub(super) struct SeqSerializer {
    items: Vec<Value>,
    all_u8: bool,
}

impl ser::SerializeSeq for SeqSerializer {
    type Ok = Value;
    type Error = PayloadError;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), PayloadError> {
        let seen = Cell::new(false);
        let value = value.serialize(ValueSerializer {
            u8_seen: Some(&seen),
        })?;
        self.all_u8 &= seen.get();
        self.items.push(value);
        Ok(())
    }
    fn end(self) -> Result<Value, PayloadError> {
        if self.all_u8 && !self.items.is_empty() {
            let bytes = self
                .items
                .iter()
                .map(|item| match item {
                    Value::Integer(n) => u8::try_from(*n).ok(),
                    _ => None,
                })
                .collect::<Option<Vec<u8>>>();
            if let Some(bytes) = bytes {
                return Ok(Value::Bytes(bytes));
            }
        }
        Ok(Value::List(self.items))
    }
}

/// Tuples and fixed-size arrays: always a list.
pub(super) struct ListSerializer(Vec<Value>);

impl ListSerializer {
    fn push<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), PayloadError> {
        self.0.push(convert(value)?);
        Ok(())
    }
}

impl ser::SerializeTuple for ListSerializer {
    type Ok = Value;
    type Error = PayloadError;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), PayloadError> {
        self.push(value)
    }
    fn end(self) -> Result<Value, PayloadError> {
        Ok(Value::List(self.0))
    }
}

impl ser::SerializeTupleStruct for ListSerializer {
    type Ok = Value;
    type Error = PayloadError;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), PayloadError> {
        self.push(value)
    }
    fn end(self) -> Result<Value, PayloadError> {
        Ok(Value::List(self.0))
    }
}

#[derive(Default)]
pub(super) struct MapSerializer {
    entries: BTreeMap<String, Value>,
    key: Option<String>,
}

impl ser::SerializeMap for MapSerializer {
    type Ok = Value;
    type Error = PayloadError;
    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), PayloadError> {
        // Strings, chars and unit enum variants are string keys.
        let Value::String(key) = convert(key)? else {
            return Err(PayloadError::MapKey);
        };
        self.key = Some(key);
        Ok(())
    }
    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), PayloadError> {
        let key = self
            .key
            .take()
            .ok_or_else(|| <PayloadError as ser::Error>::custom("map value without a key"))?;
        self.entries.insert(key, convert(value)?);
        Ok(())
    }
    fn end(self) -> Result<Value, PayloadError> {
        Ok(Value::Map(self.entries))
    }
}

impl ser::SerializeStruct for MapSerializer {
    type Ok = Value;
    type Error = PayloadError;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), PayloadError> {
        self.entries.insert(key.to_owned(), convert(value)?);
        Ok(())
    }
    fn end(self) -> Result<Value, PayloadError> {
        Ok(Value::Map(self.entries))
    }
}

/// Externally tagged enum variants: `{"Variant": content}`.
pub(super) struct VariantSerializer<T> {
    variant: &'static str,
    inner: T,
}

impl VariantSerializer<ListSerializer> {
    fn finish(self) -> Value {
        Value::Map(BTreeMap::from([(
            self.variant.to_owned(),
            Value::List(self.inner.0),
        )]))
    }
}

impl ser::SerializeTupleVariant for VariantSerializer<ListSerializer> {
    type Ok = Value;
    type Error = PayloadError;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), PayloadError> {
        self.inner.push(value)
    }
    fn end(self) -> Result<Value, PayloadError> {
        Ok(self.finish())
    }
}

impl ser::SerializeStructVariant for VariantSerializer<MapSerializer> {
    type Ok = Value;
    type Error = PayloadError;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), PayloadError> {
        ser::SerializeStruct::serialize_field(&mut self.inner, key, value)
    }
    fn end(self) -> Result<Value, PayloadError> {
        Ok(Value::Map(BTreeMap::from([(
            self.variant.to_owned(),
            Value::Map(self.inner.entries),
        )])))
    }
}
