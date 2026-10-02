//! Parser for the RE Engine class stream stored inside MHRise DSSS containers.
//!
//! The wire-format research is based in part on the MIT-licensed
//! `kvasszn/ree-save-editor` project. This implementation is intentionally
//! limited to the self-describing save payload and does not depend on its UI
//! or game asset database.

use anyhow::{Context, Result, bail};

const ARRAY_MARKER: u32 = 0xffee_ffee;
const FIELD_TYPE_ARRAY: i32 = -1;
const FIELD_TYPE_STRING: i32 = 0x0f;
const FIELD_TYPE_CLASS: i32 = 0x11;
const ARRAY_TYPE_VALUE: i32 = 0;
const ARRAY_TYPE_CLASS: i32 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavePayload {
  pub entries: Vec<NativeClass>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeClass {
  pub native_hash: u32,
  pub class: Class,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Class {
  pub hash: u32,
  pub fields: Vec<Field>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
  pub hash: u32,
  pub field_type: i32,
  pub value: FieldValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldValue {
  Array(Array),
  Class(Box<Class>),
  String(Vec<u16>),
  Scalar { size: u32, bytes: Vec<u8> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Array {
  pub member_type: i32,
  pub member_size: u32,
  pub array_type: i32,
  pub class_hashes: Option<Vec<u32>>,
  pub values: Vec<ArrayValue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArrayValue {
  Array(Box<Array>),
  Class(Box<Class>),
  String(Vec<u16>),
  Scalar(Vec<u8>),
}

impl SavePayload {
  pub fn parse(data: &[u8]) -> Result<Self> {
    Self::parse_at_offset(data, 0)
  }

  pub fn parse_at_offset(data: &[u8], alignment_offset: usize) -> Result<Self> {
    let mut reader = Reader::new(data, alignment_offset);
    let mut entries = Vec::new();
    while reader.remaining() > 0 {
      if reader.remaining() < 12 && reader.rest().iter().all(|byte| *byte == 0) {
        break;
      }
      let native_hash = reader.read_u32().context("missing native-field hash")?;
      let class = read_class(&mut reader).with_context(|| {
        format!("could not parse top-level class for native hash {native_hash:08x}")
      })?;
      entries.push(NativeClass { native_hash, class });
    }
    Ok(Self { entries })
  }

  pub fn encode(&self) -> Result<Vec<u8>> {
    self.encode_at_offset(0)
  }

  pub fn encode_at_offset(&self, alignment_offset: usize) -> Result<Vec<u8>> {
    let mut writer = Writer::new(alignment_offset);
    for entry in &self.entries {
      writer.write_u32(entry.native_hash);
      write_class(&mut writer, &entry.class)?;
    }
    Ok(writer.finish())
  }
}

fn read_class(reader: &mut Reader<'_>) -> Result<Class> {
  let field_count = reader.read_u32()? as usize;
  let hash = reader.read_u32()?;
  if field_count > reader.remaining() / 8 {
    bail!("class {hash:08x} declares an impossible field count of {field_count}");
  }
  let mut fields = Vec::with_capacity(field_count);
  for index in 0..field_count {
    let field = read_field(reader)
      .with_context(|| format!("could not parse field {index} of class {hash:08x}"))?;
    fields.push(field);
  }
  Ok(Class { hash, fields })
}

fn read_field(reader: &mut Reader<'_>) -> Result<Field> {
  let offset = reader.position();
  let hash = reader.read_u32()?;
  let field_type = reader.read_i32()?;
  let value = (|| {
    Ok::<FieldValue, anyhow::Error>(match field_type {
      FIELD_TYPE_ARRAY => FieldValue::Array(read_array(reader)?),
      FIELD_TYPE_CLASS => FieldValue::Class(Box::new(read_class(reader)?)),
      FIELD_TYPE_STRING => FieldValue::String(read_string(reader)?),
      _ => {
        reader.align(4)?;
        let size = reader.read_u32()?;
        FieldValue::Scalar { size, bytes: read_sized(reader, field_type, size)? }
      }
    })
  })()
  .with_context(|| {
    format!("could not parse field {hash:08x} type {field_type} at offset {offset:#x}")
  })?;
  reader.align(4)?;
  Ok(Field { hash, field_type, value })
}

fn read_array(reader: &mut Reader<'_>) -> Result<Array> {
  reader.align(4)?;
  let member_type = reader.read_i32()?;
  let member_size = reader.read_u32()?;
  let len = reader.read_u32()? as usize;
  let array_type = reader.read_i32()?;
  if array_type != ARRAY_TYPE_VALUE && array_type != ARRAY_TYPE_CLASS {
    bail!("unsupported array type {array_type}");
  }

  let class_hashes = if array_type == ARRAY_TYPE_CLASS && reader.peek_u32() == Some(ARRAY_MARKER) {
    reader.read_u32()?;
    let mut hashes = Vec::with_capacity(len);
    for _ in 0..len {
      hashes.push(reader.read_u32()?);
    }
    Some(hashes)
  } else {
    None
  };

  let mut values = Vec::with_capacity(len);
  for _ in 0..len {
    let value = if array_type == ARRAY_TYPE_CLASS {
      ArrayValue::Class(Box::new(read_class(reader)?))
    } else {
      match member_type {
        FIELD_TYPE_ARRAY => ArrayValue::Array(Box::new(read_array(reader)?)),
        FIELD_TYPE_STRING => ArrayValue::String(read_string(reader)?),
        _ => ArrayValue::Scalar(read_sized(reader, member_type, member_size)?),
      }
    };
    values.push(value);
  }
  reader.align(4)?;
  Ok(Array { member_type, member_size, array_type, class_hashes, values })
}

fn read_string(reader: &mut Reader<'_>) -> Result<Vec<u16>> {
  reader.align(4)?;
  let len = reader.read_u32()? as usize;
  if len > reader.remaining() / 2 {
    bail!("string length {len} exceeds the remaining payload");
  }
  let mut value = Vec::with_capacity(len);
  for _ in 0..len {
    value.push(reader.read_u16()?);
  }
  Ok(value)
}

fn read_sized(reader: &mut Reader<'_>, field_type: i32, size: u32) -> Result<Vec<u8>> {
  if size == 0 {
    bail!("zero-sized value for field type {field_type}");
  }
  if size != 1 {
    reader.align_sized(size as usize)?;
  }
  reader.read_bytes(size as usize)
}

fn write_class(writer: &mut Writer, class: &Class) -> Result<()> {
  writer.write_u32(class.fields.len().try_into().context("too many class fields")?);
  writer.write_u32(class.hash);
  for field in &class.fields {
    write_field(writer, field)?;
  }
  Ok(())
}

fn write_field(writer: &mut Writer, field: &Field) -> Result<()> {
  writer.write_u32(field.hash);
  writer.write_i32(field.field_type);
  match &field.value {
    FieldValue::Array(array) if field.field_type == FIELD_TYPE_ARRAY => {
      write_array(writer, array)?;
    }
    FieldValue::Class(class) if field.field_type == FIELD_TYPE_CLASS => {
      write_class(writer, class)?;
    }
    FieldValue::String(value) if field.field_type == FIELD_TYPE_STRING => {
      write_string(writer, value)?;
    }
    FieldValue::Scalar { size, bytes } => {
      writer.align(4);
      writer.write_u32(*size);
      write_sized(writer, *size, bytes)?;
    }
    _ => bail!("field {:08x} has a value that does not match its type", field.hash),
  }
  writer.align(4);
  Ok(())
}

fn write_array(writer: &mut Writer, array: &Array) -> Result<()> {
  writer.align(4);
  writer.write_i32(array.member_type);
  writer.write_u32(array.member_size);
  writer.write_u32(array.values.len().try_into().context("array is too large")?);
  writer.write_i32(array.array_type);

  if let Some(hashes) = &array.class_hashes {
    if array.array_type != ARRAY_TYPE_CLASS || hashes.len() != array.values.len() {
      bail!("class-array hash table does not match its values");
    }
    writer.write_u32(ARRAY_MARKER);
    for hash in hashes {
      writer.write_u32(*hash);
    }
  }

  for value in &array.values {
    match value {
      ArrayValue::Array(value)
        if array.array_type == ARRAY_TYPE_VALUE && array.member_type == FIELD_TYPE_ARRAY =>
      {
        write_array(writer, value)?;
      }
      ArrayValue::Class(value) if array.array_type == ARRAY_TYPE_CLASS => {
        write_class(writer, value)?;
      }
      ArrayValue::String(value)
        if array.array_type == ARRAY_TYPE_VALUE && array.member_type == FIELD_TYPE_STRING =>
      {
        write_string(writer, value)?;
      }
      ArrayValue::Scalar(value) if array.array_type == ARRAY_TYPE_VALUE => {
        write_sized(writer, array.member_size, value)?;
      }
      _ => bail!("array value does not match its declared member type"),
    }
  }
  writer.align(4);
  Ok(())
}

fn write_string(writer: &mut Writer, value: &[u16]) -> Result<()> {
  writer.align(4);
  writer.write_u32(value.len().try_into().context("string is too long")?);
  for code_unit in value {
    writer.write_u16(*code_unit);
  }
  Ok(())
}

fn write_sized(writer: &mut Writer, size: u32, bytes: &[u8]) -> Result<()> {
  if size == 0 || bytes.len() != size as usize {
    bail!("sized value declares {size} bytes but contains {}", bytes.len());
  }
  if size != 1 {
    writer.align_sized(size as usize)?;
  }
  writer.write_bytes(bytes);
  Ok(())
}

struct Reader<'a> {
  data: &'a [u8],
  offset: usize,
  alignment_offset: usize,
}

impl<'a> Reader<'a> {
  fn new(data: &'a [u8], alignment_offset: usize) -> Self {
    Self { data, offset: 0, alignment_offset }
  }

  fn remaining(&self) -> usize {
    self.data.len().saturating_sub(self.offset)
  }

  fn position(&self) -> usize {
    self.offset
  }

  fn rest(&self) -> &'a [u8] {
    &self.data[self.offset..]
  }

  fn align(&mut self, alignment: usize) -> Result<()> {
    if !alignment.is_power_of_two() {
      bail!("unsupported non-power-of-two alignment {alignment}");
    }
    let absolute = self.alignment_offset.checked_add(self.offset).context("offset overflow")?;
    let aligned =
      absolute.checked_add(alignment - 1).context("offset overflow")? & !(alignment - 1);
    self.offset = aligned.checked_sub(self.alignment_offset).context("invalid alignment origin")?;
    if self.offset > self.data.len() {
      bail!("alignment exceeds payload bounds");
    }
    Ok(())
  }

  fn align_sized(&mut self, size: usize) -> Result<()> {
    let previous = self.offset;
    let absolute = self.alignment_offset.checked_add(self.offset).context("offset overflow")?;
    let aligned = absolute.checked_add(size - 1).context("offset overflow")? & !(size - 1);
    self.offset = aligned.checked_sub(self.alignment_offset).context("invalid alignment origin")?;
    if self.offset > self.data.len() {
      bail!(
        "sized-value alignment from {previous:#x} to {:#x} for size {size:#x} exceeds payload length {:#x}",
        self.offset,
        self.data.len()
      );
    }
    Ok(())
  }

  fn read_bytes(&mut self, len: usize) -> Result<Vec<u8>> {
    let end = self.offset.checked_add(len).context("offset overflow")?;
    let value = self.data.get(self.offset..end).context("unexpected end of payload")?.to_vec();
    self.offset = end;
    Ok(value)
  }

  fn read_u16(&mut self) -> Result<u16> {
    Ok(u16::from_le_bytes(self.read_bytes(2)?.try_into().expect("fixed-size value")))
  }

  fn read_u32(&mut self) -> Result<u32> {
    Ok(u32::from_le_bytes(self.read_bytes(4)?.try_into().expect("fixed-size value")))
  }

  fn read_i32(&mut self) -> Result<i32> {
    Ok(i32::from_le_bytes(self.read_bytes(4)?.try_into().expect("fixed-size value")))
  }

  fn peek_u32(&self) -> Option<u32> {
    Some(u32::from_le_bytes(self.data.get(self.offset..self.offset + 4)?.try_into().ok()?))
  }
}

struct Writer {
  data: Vec<u8>,
  alignment_offset: usize,
}

impl Writer {
  fn new(alignment_offset: usize) -> Self {
    Self { data: Vec::new(), alignment_offset }
  }

  fn align(&mut self, alignment: usize) {
    debug_assert!(alignment.is_power_of_two());
    let absolute = self.alignment_offset + self.data.len();
    let aligned = absolute.div_ceil(alignment) * alignment;
    let len = aligned - self.alignment_offset;
    self.data.resize(len, 0);
  }

  fn align_sized(&mut self, size: usize) -> Result<()> {
    let absolute = self.alignment_offset.checked_add(self.data.len()).context("offset overflow")?;
    let aligned = absolute.checked_add(size - 1).context("offset overflow")? & !(size - 1);
    let len = aligned.checked_sub(self.alignment_offset).context("invalid alignment origin")?;
    self.data.resize(len, 0);
    Ok(())
  }

  fn write_bytes(&mut self, value: &[u8]) {
    self.data.extend_from_slice(value);
  }

  fn write_u16(&mut self, value: u16) {
    self.write_bytes(&value.to_le_bytes());
  }

  fn write_u32(&mut self, value: u32) {
    self.write_bytes(&value.to_le_bytes());
  }

  fn write_i32(&mut self, value: i32) {
    self.write_bytes(&value.to_le_bytes());
  }

  fn finish(self) -> Vec<u8> {
    self.data
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn parses_and_reencodes_nested_payload() {
    let payload = SavePayload {
      entries: vec![NativeClass {
        native_hash: 0x1122_3344,
        class: Class {
          hash: 0x5566_7788,
          fields: vec![
            Field {
              hash: 1,
              field_type: 8,
              value: FieldValue::Scalar { size: 4, bytes: 42u32.to_le_bytes().to_vec() },
            },
            Field {
              hash: 2,
              field_type: FIELD_TYPE_STRING,
              value: FieldValue::String("Rise".encode_utf16().collect()),
            },
            Field {
              hash: 3,
              field_type: FIELD_TYPE_ARRAY,
              value: FieldValue::Array(Array {
                member_type: 4,
                member_size: 1,
                array_type: ARRAY_TYPE_VALUE,
                class_hashes: None,
                values: vec![ArrayValue::Scalar(vec![1]), ArrayValue::Scalar(vec![2])],
              }),
            },
            Field {
              hash: 4,
              field_type: 0x10,
              value: FieldValue::Scalar { size: 5, bytes: vec![1, 2, 3, 4, 5] },
            },
          ],
        },
      }],
    };

    for offset in [0, 12, 16] {
      let encoded = payload.encode_at_offset(offset).expect("fixture should encode");
      assert_eq!(
        SavePayload::parse_at_offset(&encoded, offset).expect("fixture should parse"),
        payload
      );
    }
  }
}
