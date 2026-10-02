//! Lossless JSON representation of the RE Engine class stream.
//!
//! [`SavePayload`](crate::payload::SavePayload) converts to a
//! [`JsonSave`] tree whose hashes and field types are hex strings, scalars
//! keep their exact bytes in `hex`, and UTF-16 strings keep their code units
//! in `units`. Human-friendly `value` / `f32` fields are display-only and are
//! ignored when loading, so a dump → load round trip is always lossless.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::payload::{Array, ArrayValue, Class, Field, FieldValue, NativeClass, SavePayload};

pub const JSON_FORMAT_ID: &str = "mhrise-save-editor";
pub const JSON_FORMAT_VERSION: u32 = 1;

const FIELD_TYPE_ARRAY: i32 = -1;
const FIELD_TYPE_STRING: i32 = 0x0f;
const FIELD_TYPE_CLASS: i32 = 0x11;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonSave {
  pub format: String,
  pub version: u32,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub steamid64: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub curve_index: Option<usize>,
  pub payload: JsonPayload,
}

impl JsonSave {
  pub fn from_payload(
    payload: &SavePayload,
    steamid64: Option<u64>,
    curve_index: Option<usize>,
  ) -> Self {
    JsonSave {
      format: JSON_FORMAT_ID.to_owned(),
      version: JSON_FORMAT_VERSION,
      steamid64: steamid64.map(|id| id.to_string()),
      curve_index,
      payload: JsonPayload::from_payload(payload),
    }
  }

  /// Validates and converts the JSON tree back into a payload.
  pub fn to_payload(&self) -> Result<SavePayload> {
    if self.format != JSON_FORMAT_ID {
      bail!("json file has format {:?}, expected {:?}", self.format, JSON_FORMAT_ID);
    }
    if self.version != JSON_FORMAT_VERSION {
      bail!("json file has unsupported version {}", self.version);
    }
    self.payload.to_payload()
  }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonPayload {
  pub entries: Vec<JsonNativeClass>,
}

impl JsonPayload {
  fn from_payload(payload: &SavePayload) -> Self {
    JsonPayload {
      entries: payload
        .entries
        .iter()
        .map(|entry| JsonNativeClass {
          native_hash: hex_u32(entry.native_hash),
          class: JsonClass::from_class(&entry.class),
        })
        .collect(),
    }
  }

  fn to_payload(&self) -> Result<SavePayload> {
    let entries = self
      .entries
      .iter()
      .enumerate()
      .map(|(index, entry)| {
        Ok(NativeClass {
          native_hash: parse_hex_u32(&entry.native_hash)
            .with_context(|| format!("entry {index}: invalid native hash"))?,
          class: entry.class.to_class().with_context(|| format!("entry {index}: invalid class"))?,
        })
      })
      .collect::<Result<Vec<_>>>()?;
    Ok(SavePayload { entries })
  }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonNativeClass {
  pub native_hash: String,
  pub class: JsonClass,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonClass {
  pub hash: String,
  pub fields: Vec<JsonField>,
}

impl JsonClass {
  fn from_class(class: &Class) -> Self {
    JsonClass {
      hash: hex_u32(class.hash),
      fields: class.fields.iter().map(JsonField::from_field).collect(),
    }
  }

  fn to_class(&self) -> Result<Class> {
    let hash = parse_hex_u32(&self.hash).context("invalid class hash")?;
    let fields = self
      .fields
      .iter()
      .enumerate()
      .map(|(index, field)| {
        field.to_field().with_context(|| format!("class {hash:08x}: invalid field {index}"))
      })
      .collect::<Result<Vec<_>>>()?;
    Ok(Class { hash, fields })
  }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonField {
  pub hash: String,
  #[serde(rename = "type")]
  pub field_type: String,
  pub value: JsonValue,
}

impl JsonField {
  fn from_field(field: &Field) -> Self {
    let value = match &field.value {
      FieldValue::Array(array) => JsonValue::Array(JsonArray::from_array(array)),
      FieldValue::Class(class) => JsonValue::Class(JsonClass::from_class(class)),
      FieldValue::String(units) => {
        JsonValue::String { units: units.clone(), value: Some(String::from_utf16_lossy(units)) }
      }
      FieldValue::Scalar { size, bytes } => JsonValue::Scalar {
        size: *size,
        hex: hex_bytes(bytes),
        value: scalar_display(*size, bytes),
      },
    };
    JsonField { hash: hex_u32(field.hash), field_type: hex_i32(field.field_type), value }
  }

  fn to_field(&self) -> Result<Field> {
    let hash = parse_hex_u32(&self.hash)?;
    let field_type = parse_type_i32(&self.field_type)
      .with_context(|| format!("field {hash:08x}: invalid type"))?;
    let value = match &self.value {
      JsonValue::Array(array) => {
        if field_type != FIELD_TYPE_ARRAY {
          bail!("field {hash:08x}: json value is an array but the type is {:#x}", field_type);
        }
        FieldValue::Array(array.to_array()?)
      }
      JsonValue::Class(class) => {
        if field_type != FIELD_TYPE_CLASS {
          bail!("field {hash:08x}: json value is a class but the type is {:#x}", field_type);
        }
        FieldValue::Class(Box::new(class.to_class()?))
      }
      JsonValue::String { units, .. } => {
        if field_type != FIELD_TYPE_STRING {
          bail!("field {hash:08x}: json value is a string but the type is {:#x}", field_type);
        }
        FieldValue::String(units.clone())
      }
      JsonValue::Scalar { size, hex, value } => {
        let bytes =
          decode_hex_bytes(hex).with_context(|| format!("field {hash:08x}: invalid bytes"))?;
        if bytes.len() != *size as usize {
          bail!(
            "field {hash:08x}: hex payload is {} bytes but the size field says {size}",
            bytes.len()
          );
        }
        validate_scalar_display(size, &bytes, value)
          .with_context(|| format!("field {hash:08x}: display fields disagree with hex"))?;
        FieldValue::Scalar { size: *size, bytes }
      }
    };
    Ok(Field { hash, field_type, value })
  }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JsonValue {
  Scalar {
    size: u32,
    hex: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    value: Option<String>,
  },
  String {
    units: Vec<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    value: Option<String>,
  },
  Class(JsonClass),
  Array(JsonArray),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonArray {
  #[serde(rename = "member_type")]
  pub member_type: String,
  pub member_size: u32,
  pub array_type: i32,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub class_hashes: Option<Vec<String>>,
  pub values: Vec<JsonArrayValue>,
}

impl JsonArray {
  fn from_array(array: &Array) -> Self {
    let values = array
      .values
      .iter()
      .map(|value| match value {
        ArrayValue::Array(inner) => JsonArrayValue::Array(JsonArray::from_array(inner)),
        ArrayValue::Class(class) => JsonArrayValue::Class(JsonClass::from_class(class)),
        ArrayValue::String(units) => JsonArrayValue::String {
          units: units.clone(),
          value: Some(String::from_utf16_lossy(units)),
        },
        ArrayValue::Scalar(bytes) => JsonArrayValue::Scalar {
          hex: hex_bytes(bytes),
          value: scalar_display(array.member_size, bytes),
        },
      })
      .collect();
    JsonArray {
      member_type: hex_i32(array.member_type),
      member_size: array.member_size,
      array_type: array.array_type,
      class_hashes: array
        .class_hashes
        .as_ref()
        .map(|hashes| hashes.iter().map(|hash| hex_u32(*hash)).collect()),
      values,
    }
  }

  fn to_array(&self) -> Result<Array> {
    let member_type = parse_type_i32(&self.member_type).context("array: invalid member type")?;
    let class_hashes = match &self.class_hashes {
      Some(hashes) => Some(
        hashes
          .iter()
          .map(|text| parse_hex_u32(text).context("array: invalid class hash"))
          .collect::<Result<Vec<_>>>()?,
      ),
      None => None,
    };
    let values = self
      .values
      .iter()
      .enumerate()
      .map(|(index, value)| {
        value
          .to_array_value(self.member_size)
          .with_context(|| format!("array: invalid value {index}"))
      })
      .collect::<Result<Vec<_>>>()?;
    Ok(Array {
      member_type,
      member_size: self.member_size,
      array_type: self.array_type,
      class_hashes,
      values,
    })
  }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JsonArrayValue {
  Scalar {
    hex: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    value: Option<String>,
  },
  String {
    units: Vec<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    value: Option<String>,
  },
  Class(JsonClass),
  Array(JsonArray),
}

impl JsonArrayValue {
  fn to_array_value(&self, member_size: u32) -> Result<ArrayValue> {
    Ok(match self {
      JsonArrayValue::Scalar { hex, value } => {
        let bytes = decode_hex_bytes(hex).context("array scalar: invalid bytes")?;
        validate_scalar_display(&member_size, &bytes, value)
          .context("array scalar: display fields disagree with hex")?;
        ArrayValue::Scalar(bytes)
      }
      JsonArrayValue::String { units, .. } => ArrayValue::String(units.clone()),
      JsonArrayValue::Class(class) => ArrayValue::Class(Box::new(class.to_class()?)),
      JsonArrayValue::Array(array) => ArrayValue::Array(Box::new(array.to_array()?)),
    })
  }
}

/// `value` is the unsigned-integer view of the authoritative `hex` bytes. On
/// load it must still match those bytes; otherwise the JSON was edited in the
/// wrong field, and silently ignoring the edit would lose user work.
fn validate_scalar_display(size: &u32, bytes: &[u8], value: &Option<String>) -> Result<()> {
  let Some(text) = value else { return Ok(()) };
  let text = text.trim();
  let parsed = text
    .parse::<u64>()
    .with_context(|| format!("display value {text:?} is not a plain unsigned integer"))?;
  let limit = match *size {
    1 => u64::from(u8::MAX),
    2 => u64::from(u16::MAX),
    4 => u64::from(u32::MAX),
    8 => u64::MAX,
    other => bail!("display value is not available for {other}-byte scalars"),
  };
  if parsed > limit {
    bail!("display value {parsed} does not fit in {size} byte(s)");
  }
  if parsed.to_le_bytes()[..bytes.len()] != *bytes {
    bail!(
      "display value {parsed} does not match the hex bytes {}; edit hex, which is authoritative, and keep or drop the display value",
      hex_bytes(bytes)
    );
  }
  Ok(())
}

fn hex_u32(value: u32) -> String {
  format!("0x{value:08x}")
}

fn hex_i32(value: i32) -> String {
  if value < 0 { format!("{value}") } else { format!("0x{value:08x}") }
}

fn hex_bytes(bytes: &[u8]) -> String {
  bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn parse_hex_u32(text: &str) -> Result<u32> {
  let text = text.trim();
  let digits = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")).unwrap_or(text);
  u32::from_str_radix(digits, 16).with_context(|| format!("invalid hex value {text:?}"))
}

fn parse_type_i32(text: &str) -> Result<i32> {
  let text = text.trim();
  let (negative, rest) = match text.strip_prefix('-') {
    Some(digits) => (true, digits),
    None => (false, text),
  };
  let magnitude = if let Some(digits) = rest.strip_prefix("0x").or_else(|| rest.strip_prefix("0X"))
  {
    u32::from_str_radix(digits, 16).with_context(|| format!("invalid type {text:?}"))?
  } else if let Ok(decimal) = rest.parse::<u32>() {
    decimal
  } else {
    u32::from_str_radix(rest, 16).with_context(|| format!("invalid type {text:?}"))?
  };
  if negative { Ok(-(magnitude as i32)) } else { Ok(magnitude as i32) }
}

fn decode_hex_bytes(text: &str) -> Result<Vec<u8>> {
  let text = text.trim();
  let digits = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")).unwrap_or(text);
  if !digits.len().is_multiple_of(2) {
    bail!("hex byte string has an odd length: {text:?}");
  }
  (0..digits.len())
    .step_by(2)
    .map(|index| {
      u8::from_str_radix(&digits[index..index + 2], 16)
        .with_context(|| format!("invalid hex byte string {text:?}"))
    })
    .collect()
}

fn scalar_display(size: u32, bytes: &[u8]) -> Option<String> {
  match size {
    1 => bytes.try_into().ok().map(u8::from_le_bytes).map(|value| value.to_string()),
    2 => bytes.try_into().ok().map(u16::from_le_bytes).map(|value| value.to_string()),
    4 => bytes.try_into().ok().map(u32::from_le_bytes).map(|value| value.to_string()),
    8 => bytes.try_into().ok().map(u64::from_le_bytes).map(|value| value.to_string()),
    _ => None,
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn sample_payload() -> SavePayload {
    SavePayload {
      entries: vec![NativeClass {
        native_hash: 0x85e9_04c1,
        class: Class {
          hash: 0x5766_f30b,
          fields: vec![
            Field {
              hash: 0x0a96_0102,
              field_type: 0x08,
              value: FieldValue::Scalar { size: 4, bytes: 1234u32.to_le_bytes().to_vec() },
            },
            Field {
              hash: 0x0b96_0102,
              field_type: 0x09,
              value: FieldValue::Scalar { size: 8, bytes: (-5i64).to_le_bytes().to_vec() },
            },
            Field {
              hash: 0x0c96_0102,
              field_type: 0x0a,
              value: FieldValue::Scalar { size: 4, bytes: 1.5f32.to_le_bytes().to_vec() },
            },
            Field {
              hash: 0x0d96_0102,
              field_type: 0x04,
              value: FieldValue::Scalar { size: 1, bytes: vec![0xff] },
            },
            Field {
              hash: 0x1234_5678,
              field_type: FIELD_TYPE_STRING,
              value: FieldValue::String("ハンター".encode_utf16().collect()),
            },
            Field {
              hash: 0x8765_4321,
              field_type: FIELD_TYPE_CLASS,
              value: FieldValue::Class(Box::new(Class {
                hash: 0xfeed_face,
                fields: vec![Field {
                  hash: 0x9999_0001,
                  field_type: 0x02,
                  value: FieldValue::Scalar { size: 1, bytes: vec![1] },
                }],
              })),
            },
            Field {
              hash: 0xabcd_ef01,
              field_type: FIELD_TYPE_ARRAY,
              value: FieldValue::Array(Array {
                member_type: 0x08,
                member_size: 4,
                array_type: 0,
                class_hashes: None,
                values: (0..3u32).map(|i| ArrayValue::Scalar(i.to_le_bytes().to_vec())).collect(),
              }),
            },
            Field {
              hash: 0xabcd_ef02,
              field_type: FIELD_TYPE_ARRAY,
              value: FieldValue::Array(Array {
                member_type: 0x11,
                member_size: 0,
                array_type: 1,
                class_hashes: Some(vec![0xaaaa_0001]),
                values: vec![ArrayValue::Class(Box::new(Class {
                  hash: 0xaaaa_0001,
                  fields: vec![
                    Field {
                      hash: 0xaaaa_0002,
                      field_type: FIELD_TYPE_ARRAY,
                      value: FieldValue::Array(Array {
                        member_type: FIELD_TYPE_ARRAY,
                        member_size: 0,
                        array_type: 0,
                        class_hashes: None,
                        values: vec![ArrayValue::Array(Box::new(Array {
                          member_type: 0x04,
                          member_size: 1,
                          array_type: 0,
                          class_hashes: None,
                          values: vec![ArrayValue::Scalar(vec![1]), ArrayValue::Scalar(vec![2])],
                        }))],
                      }),
                    },
                    Field {
                      hash: 0xaaaa_0003,
                      field_type: FIELD_TYPE_STRING,
                      value: FieldValue::String("nested".encode_utf16().collect()),
                    },
                  ],
                }))],
              }),
            },
          ],
        },
      }],
    }
  }

  #[test]
  fn json_roundtrip_is_lossless() {
    let payload = sample_payload();
    let json = JsonSave::from_payload(&payload, Some(76_561_198_382_766_028), Some(12));
    let text = serde_json::to_string_pretty(&json).expect("json should serialize");
    let parsed: JsonSave = serde_json::from_str(&text).expect("json should deserialize");
    assert_eq!(parsed, json);
    assert_eq!(parsed.to_payload().expect("json should convert"), payload);
  }

  #[test]
  fn stale_display_fields_are_rejected() {
    let payload = sample_payload();
    let json = JsonSave::from_payload(&payload, None, None);
    let text = serde_json::to_string(&json).expect("json should serialize");
    // A user edits the display value instead of hex: the loader must refuse
    // loudly instead of silently dropping the edit.
    let edited = text.replace("\"value\":\"1234\"", "\"value\":\"0\"");
    assert_ne!(text, edited, "fixture display field should appear in the dump");
    let parsed: JsonSave = serde_json::from_str(&edited).expect("edited json should deserialize");
    let error = parsed.to_payload().expect_err("stale display fields should be rejected");
    assert!(format!("{error:?}").contains("authoritative"), "unexpected error: {error:?}");
  }

  #[test]
  fn consistent_hex_and_display_edits_are_accepted() {
    let payload = sample_payload();
    let json = JsonSave::from_payload(&payload, None, None);
    let text = serde_json::to_string(&json).expect("json should serialize");
    let edited = text
      .replace("\"hex\":\"d2040000\"", "\"hex\":\"0f270000\"")
      .replace("\"value\":\"1234\"", "\"value\":\"9999\"");
    let parsed: JsonSave = serde_json::from_str(&edited).expect("edited json should deserialize");
    let loaded = parsed.to_payload().expect("consistent edit should convert");
    let FieldValue::Scalar { bytes, .. } = &loaded.entries[0].class.fields[0].value else {
      panic!("first field should be a scalar");
    };
    assert_eq!(bytes.as_slice(), 9999u32.to_le_bytes());
  }

  #[test]
  fn rejects_mismatched_value_kind() {
    let text = r#"{
      "format": "mhrise-save-editor",
      "version": 1,
      "payload": { "entries": [ { "native_hash": "0x00000001", "class": { "hash": "0x00000002", "fields": [ {
        "hash": "0x00000003",
        "type": "0x08",
        "value": { "kind": "string", "units": [] }
      } ] } } ] } }"#;
    let parsed: JsonSave = serde_json::from_str(text).expect("fixture json should deserialize");
    let error = parsed.to_payload().expect_err("string value under scalar type should fail");
    assert!(format!("{error:?}").contains("json value is a string"), "unexpected error: {error:?}");
  }

  #[test]
  fn rejects_wrong_format_id() {
    let text = r#"{ "format": "other-tool", "version": 1, "payload": { "entries": [] } }"#;
    let parsed: JsonSave = serde_json::from_str(text).expect("fixture json should deserialize");
    let error = parsed.to_payload().expect_err("wrong format id should fail");
    assert!(error.to_string().contains("format"), "unexpected error: {error}");
  }
}
