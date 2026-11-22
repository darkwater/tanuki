//! A Serde deserializer reading from a borrowed [`Value`].
use super::PayloadError;
use crate::Value;
use serde::de::{
    self, DeserializeSeed, EnumAccess, IntoDeserializer, Unexpected, VariantAccess, Visitor,
    value::{MapDeserializer, SeqDeserializer},
};

pub(super) struct ValueDeserializer<'de>(pub(super) &'de Value);

/// A list element or map value, for Serde's sequence and map adapters.
struct Element<'de>(&'de Value);

impl<'de> IntoDeserializer<'de, PayloadError> for Element<'de> {
    type Deserializer = ValueDeserializer<'de>;
    fn into_deserializer(self) -> ValueDeserializer<'de> {
        ValueDeserializer(self.0)
    }
}

impl ValueDeserializer<'_> {
    fn unexpected(&self) -> Unexpected<'_> {
        match self.0 {
            Value::Null => Unexpected::Unit,
            Value::Bool(v) => Unexpected::Bool(*v),
            Value::Integer(v) => Unexpected::Signed(*v),
            Value::Float(v) => Unexpected::Float(v.get()),
            Value::String(v) => Unexpected::Str(v),
            Value::Bytes(v) => Unexpected::Bytes(v),
            Value::List(_) => Unexpected::Seq,
            Value::Map(_) => Unexpected::Map,
        }
    }
}

fn visit_list<'de, V: Visitor<'de>>(
    items: &'de [Value],
    visitor: V,
) -> Result<V::Value, PayloadError> {
    let mut seq = SeqDeserializer::new(items.iter().map(Element));
    let value = visitor.visit_seq(&mut seq)?;
    seq.end()?;
    Ok(value)
}

/// Bytes read as a sequence of `u8`, so `Vec<u8>` and `[u8; N]` accept them.
fn visit_bytes_as_seq<'de, V: Visitor<'de>>(
    bytes: &'de [u8],
    visitor: V,
) -> Result<V::Value, PayloadError> {
    let mut seq = SeqDeserializer::<_, PayloadError>::new(bytes.iter().copied());
    let value = visitor.visit_seq(&mut seq)?;
    seq.end()?;
    Ok(value)
}

impl<'de> de::Deserializer<'de> for ValueDeserializer<'de> {
    type Error = PayloadError;

    fn is_human_readable(&self) -> bool {
        false
    }

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, PayloadError> {
        match self.0 {
            Value::Null => visitor.visit_unit(),
            Value::Bool(v) => visitor.visit_bool(*v),
            Value::Integer(v) => visitor.visit_i64(*v),
            Value::Float(v) => visitor.visit_f64(v.get()),
            Value::String(v) => visitor.visit_borrowed_str(v),
            Value::Bytes(v) => visitor.visit_borrowed_bytes(v),
            Value::List(items) => visit_list(items, visitor),
            Value::Map(entries) => {
                let mut map = MapDeserializer::new(
                    entries
                        .iter()
                        .map(|(key, value)| (key.as_str(), Element(value))),
                );
                let value = visitor.visit_map(&mut map)?;
                map.end()?;
                Ok(value)
            }
        }
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, PayloadError> {
        match self.0 {
            Value::Null => visitor.visit_none(),
            _ => visitor.visit_some(self),
        }
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, PayloadError> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, PayloadError> {
        match self.0 {
            Value::Bytes(bytes) => visit_bytes_as_seq(bytes, visitor),
            _ => self.deserialize_any(visitor),
        }
    }

    fn deserialize_tuple<V: Visitor<'de>>(
        self,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value, PayloadError> {
        self.deserialize_seq(visitor)
    }

    fn deserialize_tuple_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value, PayloadError> {
        self.deserialize_seq(visitor)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, PayloadError> {
        match self.0 {
            Value::String(variant) => visitor.visit_enum(
                IntoDeserializer::<PayloadError>::into_deserializer(variant.as_str()),
            ),
            Value::Map(entries) if entries.len() == 1 => {
                let (variant, content) = entries.iter().next().expect("one entry");
                visitor.visit_enum(Variant { variant, content })
            }
            _ => Err(de::Error::invalid_type(self.unexpected(), &visitor)),
        }
    }

    fn deserialize_ignored_any<V: Visitor<'de>>(
        self,
        visitor: V,
    ) -> Result<V::Value, PayloadError> {
        visitor.visit_unit()
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf unit unit_struct map struct identifier
    }
}

/// An externally tagged variant `{"Variant": content}`.
struct Variant<'de> {
    variant: &'de str,
    content: &'de Value,
}

impl<'de> EnumAccess<'de> for Variant<'de> {
    type Error = PayloadError;
    type Variant = Self;
    fn variant_seed<S: DeserializeSeed<'de>>(
        self,
        seed: S,
    ) -> Result<(S::Value, Self), PayloadError> {
        let variant = seed.deserialize(IntoDeserializer::<PayloadError>::into_deserializer(
            self.variant,
        ))?;
        Ok((variant, self))
    }
}

impl<'de> VariantAccess<'de> for Variant<'de> {
    type Error = PayloadError;
    fn unit_variant(self) -> Result<(), PayloadError> {
        match self.content {
            Value::Null => Ok(()),
            _ => Err(de::Error::invalid_type(
                ValueDeserializer(self.content).unexpected(),
                &"a unit variant",
            )),
        }
    }
    fn newtype_variant_seed<S: DeserializeSeed<'de>>(
        self,
        seed: S,
    ) -> Result<S::Value, PayloadError> {
        seed.deserialize(ValueDeserializer(self.content))
    }
    fn tuple_variant<V: Visitor<'de>>(
        self,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value, PayloadError> {
        de::Deserializer::deserialize_seq(ValueDeserializer(self.content), visitor)
    }
    fn struct_variant<V: Visitor<'de>>(
        self,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, PayloadError> {
        de::Deserializer::deserialize_any(ValueDeserializer(self.content), visitor)
    }
}
