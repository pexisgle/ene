//! End-to-end checks for the shared primitives.
//!
//! Serialization roundtrips run through a minimal in-test value model, so the
//! suite needs no wire format beyond `serde` itself. The model covers exactly
//! what the derived implementations emit: human-readable strings, `u64`
//! counters, and string-keyed struct maps.

use ene_primitive::clock::WallClockWithTz;
use ene_primitive::correlation::{DirectedPair, EmptyPurpose};
use ene_primitive::generation::GenerationInner;
use ene_primitive::raw_id::RawId;
use ene_primitive::revision::RevisionInner;
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq)]
enum MiniValue {
    /// Human-readable strings: UUID text, RFC 3339 timestamps, purposes.
    Str(String),
    /// Counter values for revisions and generations.
    U64(u64),
    Map(Vec<(String, MiniValue)>),
}

#[derive(Debug)]
struct MiniError(String);

impl core::fmt::Display for MiniError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "mini value error: {message}", message = self.0)
    }
}

impl std::error::Error for MiniError {}

impl serde::ser::Error for MiniError {
    fn custom<T: core::fmt::Display>(message: T) -> Self {
        Self(format!("{message}"))
    }
}

impl serde::de::Error for MiniError {
    fn custom<T: core::fmt::Display>(message: T) -> Self {
        Self(format!("{message}"))
    }
}

struct MiniSerializer;

struct MiniStructSerializer {
    fields: Vec<(String, MiniValue)>,
}

impl serde::Serializer for MiniSerializer {
    type Error = MiniError;
    type Ok = MiniValue;
    type SerializeMap = serde::ser::Impossible<MiniValue, MiniError>;
    type SerializeSeq = serde::ser::Impossible<MiniValue, MiniError>;
    type SerializeStruct = MiniStructSerializer;
    type SerializeStructVariant = serde::ser::Impossible<MiniValue, MiniError>;
    type SerializeTuple = serde::ser::Impossible<MiniValue, MiniError>;
    type SerializeTupleStruct = serde::ser::Impossible<MiniValue, MiniError>;
    type SerializeTupleVariant = serde::ser::Impossible<MiniValue, MiniError>;

    fn is_human_readable(&self) -> bool {
        true
    }

    fn serialize_bool(self, _value: bool) -> Result<MiniValue, MiniError> {
        Err(MiniError(
            "primitive roundtrips never encode bools".to_owned(),
        ))
    }

    fn serialize_i8(self, _value: i8) -> Result<MiniValue, MiniError> {
        Err(MiniError(
            "primitive roundtrips never encode integers".to_owned(),
        ))
    }

    fn serialize_i16(self, _value: i16) -> Result<MiniValue, MiniError> {
        Err(MiniError(
            "primitive roundtrips never encode integers".to_owned(),
        ))
    }

    fn serialize_i32(self, _value: i32) -> Result<MiniValue, MiniError> {
        Err(MiniError(
            "primitive roundtrips never encode integers".to_owned(),
        ))
    }

    fn serialize_i64(self, _value: i64) -> Result<MiniValue, MiniError> {
        Err(MiniError(
            "primitive roundtrips never encode integers".to_owned(),
        ))
    }

    fn serialize_i128(self, _value: i128) -> Result<MiniValue, MiniError> {
        Err(MiniError(
            "primitive roundtrips never encode integers".to_owned(),
        ))
    }

    fn serialize_u8(self, _value: u8) -> Result<MiniValue, MiniError> {
        Err(MiniError(
            "primitive roundtrips never encode integers".to_owned(),
        ))
    }

    fn serialize_u16(self, _value: u16) -> Result<MiniValue, MiniError> {
        Err(MiniError(
            "primitive roundtrips never encode integers".to_owned(),
        ))
    }

    fn serialize_u32(self, _value: u32) -> Result<MiniValue, MiniError> {
        Err(MiniError(
            "primitive roundtrips never encode integers".to_owned(),
        ))
    }

    fn serialize_u64(self, value: u64) -> Result<MiniValue, MiniError> {
        Ok(MiniValue::U64(value))
    }

    fn serialize_u128(self, _value: u128) -> Result<MiniValue, MiniError> {
        Err(MiniError(
            "primitive roundtrips never encode integers".to_owned(),
        ))
    }

    fn serialize_f32(self, _value: f32) -> Result<MiniValue, MiniError> {
        Err(MiniError(
            "primitive roundtrips never encode floats".to_owned(),
        ))
    }

    fn serialize_f64(self, _value: f64) -> Result<MiniValue, MiniError> {
        Err(MiniError(
            "primitive roundtrips never encode floats".to_owned(),
        ))
    }

    fn serialize_char(self, _value: char) -> Result<MiniValue, MiniError> {
        Err(MiniError(
            "primitive roundtrips never encode chars".to_owned(),
        ))
    }

    fn serialize_str(self, value: &str) -> Result<MiniValue, MiniError> {
        Ok(MiniValue::Str(value.to_owned()))
    }

    fn serialize_bytes(self, _value: &[u8]) -> Result<MiniValue, MiniError> {
        Err(MiniError(
            "primitive roundtrips never encode bytes".to_owned(),
        ))
    }

    fn serialize_none(self) -> Result<MiniValue, MiniError> {
        Err(MiniError(
            "primitive roundtrips never encode options".to_owned(),
        ))
    }

    fn serialize_some<T: serde::Serialize + ?Sized>(
        self,
        _value: &T,
    ) -> Result<MiniValue, MiniError> {
        Err(MiniError(
            "primitive roundtrips never encode options".to_owned(),
        ))
    }

    fn serialize_unit(self) -> Result<MiniValue, MiniError> {
        Err(MiniError(
            "primitive roundtrips never encode units".to_owned(),
        ))
    }

    fn serialize_unit_struct(self, _name: &'static str) -> Result<MiniValue, MiniError> {
        Err(MiniError(
            "primitive roundtrips never encode units".to_owned(),
        ))
    }

    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
    ) -> Result<MiniValue, MiniError> {
        Err(MiniError(
            "primitive roundtrips never encode enums".to_owned(),
        ))
    }

    fn serialize_newtype_struct<T: serde::Serialize + ?Sized>(
        self,
        _name: &'static str,
        value: &T,
    ) -> Result<MiniValue, MiniError> {
        value.serialize(Self)
    }

    fn serialize_newtype_variant<T: serde::Serialize + ?Sized>(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _value: &T,
    ) -> Result<MiniValue, MiniError> {
        Err(MiniError(
            "primitive roundtrips never encode enums".to_owned(),
        ))
    }

    fn serialize_seq(self, _len: Option<usize>) -> Result<Self::SerializeSeq, MiniError> {
        Err(MiniError(
            "primitive roundtrips never encode sequences".to_owned(),
        ))
    }

    fn serialize_tuple(self, _len: usize) -> Result<Self::SerializeTuple, MiniError> {
        Err(MiniError(
            "primitive roundtrips never encode tuples".to_owned(),
        ))
    }

    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleStruct, MiniError> {
        Err(MiniError(
            "primitive roundtrips never encode tuples".to_owned(),
        ))
    }

    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleVariant, MiniError> {
        Err(MiniError(
            "primitive roundtrips never encode enums".to_owned(),
        ))
    }

    fn serialize_map(self, _len: Option<usize>) -> Result<Self::SerializeMap, MiniError> {
        Err(MiniError(
            "primitive roundtrips never encode maps".to_owned(),
        ))
    }

    fn serialize_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStruct, MiniError> {
        Ok(MiniStructSerializer { fields: Vec::new() })
    }

    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStructVariant, MiniError> {
        Err(MiniError(
            "primitive roundtrips never encode enums".to_owned(),
        ))
    }

    fn collect_str<T: core::fmt::Display + ?Sized>(
        self,
        value: &T,
    ) -> Result<MiniValue, MiniError> {
        Ok(MiniValue::Str(format!("{value}")))
    }
}

impl serde::ser::SerializeStruct for MiniStructSerializer {
    type Error = MiniError;
    type Ok = MiniValue;

    fn serialize_field<T: serde::Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), MiniError> {
        let encoded = value.serialize(MiniSerializer)?;
        self.fields.push((key.to_owned(), encoded));
        Ok(())
    }

    fn end(self) -> Result<MiniValue, MiniError> {
        Ok(MiniValue::Map(self.fields))
    }
}

struct MiniDeserializer(MiniValue);

struct MiniMapAccess {
    fields: std::vec::IntoIter<(String, MiniValue)>,
    pending: Option<MiniValue>,
}

impl<'de> serde::Deserializer<'de> for MiniDeserializer {
    type Error = MiniError;

    fn is_human_readable(&self) -> bool {
        true
    }

    fn deserialize_any<V: serde::de::Visitor<'de>>(
        self,
        visitor: V,
    ) -> Result<V::Value, MiniError> {
        match self.0 {
            MiniValue::Str(text) => visitor.visit_string(text),
            MiniValue::U64(number) => visitor.visit_u64(number),
            MiniValue::Map(fields) => visitor.visit_map(MiniMapAccess {
                fields: fields.into_iter(),
                pending: None,
            }),
        }
    }

    fn deserialize_u64<V: serde::de::Visitor<'de>>(
        self,
        visitor: V,
    ) -> Result<V::Value, MiniError> {
        if let MiniValue::U64(number) = self.0 {
            visitor.visit_u64(number)
        } else {
            Err(MiniError("expected a u64 counter value".to_owned()))
        }
    }

    fn deserialize_str<V: serde::de::Visitor<'de>>(
        self,
        visitor: V,
    ) -> Result<V::Value, MiniError> {
        if let MiniValue::Str(text) = self.0 {
            visitor.visit_string(text)
        } else {
            Err(MiniError("expected a string value".to_owned()))
        }
    }

    fn deserialize_string<V: serde::de::Visitor<'de>>(
        self,
        visitor: V,
    ) -> Result<V::Value, MiniError> {
        if let MiniValue::Str(text) = self.0 {
            visitor.visit_string(text)
        } else {
            Err(MiniError("expected a string value".to_owned()))
        }
    }

    fn deserialize_option<V: serde::de::Visitor<'de>>(
        self,
        visitor: V,
    ) -> Result<V::Value, MiniError> {
        visitor.visit_some(self)
    }

    fn deserialize_identifier<V: serde::de::Visitor<'de>>(
        self,
        visitor: V,
    ) -> Result<V::Value, MiniError> {
        if let MiniValue::Str(text) = self.0 {
            visitor.visit_string(text)
        } else {
            Err(MiniError("expected a field name".to_owned()))
        }
    }

    fn deserialize_ignored_any<V: serde::de::Visitor<'de>>(
        self,
        visitor: V,
    ) -> Result<V::Value, MiniError> {
        self.deserialize_any(visitor)
    }

    fn deserialize_newtype_struct<V: serde::de::Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, MiniError> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_struct<V: serde::de::Visitor<'de>>(
        self,
        _name: &'static str,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, MiniError> {
        if let MiniValue::Map(fields) = self.0 {
            visitor.visit_map(MiniMapAccess {
                fields: fields.into_iter(),
                pending: None,
            })
        } else {
            Err(MiniError("expected a struct map".to_owned()))
        }
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u128 f32 f64 char bytes
        byte_buf unit unit_struct seq tuple tuple_struct map enum
    }
}

impl<'de> serde::de::MapAccess<'de> for MiniMapAccess {
    type Error = MiniError;

    fn next_key_seed<K: serde::de::DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, MiniError> {
        match self.fields.next() {
            Some((key, value)) => {
                self.pending = Some(value);
                seed.deserialize(MiniDeserializer(MiniValue::Str(key)))
                    .map(Some)
            }
            None => Ok(None),
        }
    }

    fn next_value_seed<V: serde::de::DeserializeSeed<'de>>(
        &mut self,
        seed: V,
    ) -> Result<V::Value, MiniError> {
        match self.pending.take() {
            Some(value) => seed.deserialize(MiniDeserializer(value)),
            None => Err(MiniError("map value missing for key".to_owned())),
        }
    }
}

fn roundtrip<T>(value: &T) -> Result<T, MiniError>
where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    let encoded = value.serialize(MiniSerializer)?;
    T::deserialize(MiniDeserializer(encoded))
}

#[test]
fn generated_ids_are_unique_across_many_draws() {
    let mut seen = HashSet::with_capacity(1024);
    for _ in 0..1000 {
        seen.insert(RawId::new());
    }
    assert_eq!(seen.len(), 1000);
}

#[test]
fn revision_advances_and_reports_exhaustion() {
    let first = RevisionInner::first();
    let Some(second) = first.checked_next() else {
        return;
    };
    assert!(first < second);
    assert_eq!(first.as_u64(), 0);
    let max = RevisionInner::from_u64(u64::MAX);
    assert_eq!(max.checked_next(), None);
}

#[test]
fn generation_advances_and_reports_exhaustion() {
    let first = GenerationInner::first();
    let Some(second) = first.checked_next() else {
        return;
    };
    assert!(first < second);
    assert_eq!(first.as_u64(), 0);
    let max = GenerationInner::from_u64(u64::MAX);
    assert_eq!(max.checked_next(), None);
}

#[test]
fn raw_id_serde_roundtrip() {
    let id = RawId::new();
    let decoded = roundtrip(&id);
    assert!(decoded.is_ok());
    if let Ok(back) = decoded {
        assert_eq!(back, id);
    }
}

#[test]
fn revision_serde_roundtrip() {
    let revision = RevisionInner::from_u64(41);
    let decoded = roundtrip(&revision);
    assert!(decoded.is_ok());
    if let Ok(back) = decoded {
        assert_eq!(back, revision);
    }
}

#[test]
fn generation_serde_roundtrip() {
    let generation = GenerationInner::from_u64(7);
    let decoded = roundtrip(&generation);
    assert!(decoded.is_ok());
    if let Ok(back) = decoded {
        assert_eq!(back, generation);
    }
}

#[test]
fn clock_serde_roundtrip() {
    let parsed = WallClockWithTz::parse_rfc3339("2026-03-14T15:09:26+05:30");
    assert!(parsed.is_ok());
    if let Ok(clock) = parsed {
        let decoded = roundtrip(&clock);
        assert!(decoded.is_ok());
        if let Ok(back) = decoded {
            assert_eq!(back, clock);
        }
    }
}

#[test]
fn directed_pair_serde_roundtrip() {
    let pair = DirectedPair::try_new(
        RawId::new(),
        RawId::new(),
        String::from("summary grounds reference the summary they explain"),
    );
    assert!(pair.is_ok());
    if let Ok(link) = pair {
        let decoded = roundtrip(&link);
        assert!(decoded.is_ok());
        if let Ok(back) = decoded {
            assert_eq!(back, link);
        }
    }
}

#[test]
fn rfc3339_roundtrip_preserves_non_utc_offset() {
    let text = "2026-03-14T15:09:26+09:00";
    let parsed = WallClockWithTz::parse_rfc3339(text);
    assert!(parsed.is_ok());
    if let Ok(clock) = parsed {
        assert_eq!(clock.to_rfc3339(), text);
    }
}

#[test]
fn directed_pair_rejects_empty_purpose() {
    assert_eq!(
        DirectedPair::try_new(RawId::new(), RawId::new(), String::new()),
        Err(EmptyPurpose)
    );
}

#[test]
fn directed_pair_keeps_direction_and_purpose() {
    let from = RawId::new();
    let to = RawId::new();
    let pair = DirectedPair::try_new(from, to, String::from("retry supersedes attempt"));
    assert!(pair.is_ok());
    if let Ok(link) = pair {
        assert_eq!(link.from_raw, from);
        assert_eq!(link.to_raw, to);
        assert_eq!(link.purpose, "retry supersedes attempt");
    }
}
