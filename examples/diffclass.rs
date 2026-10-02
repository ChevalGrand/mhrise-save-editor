//! Debug helper: classifies every byte difference between the decrypted
//! payload of a save and its re-encode as DATA (consumed by the parser as part
//! of a value/header) or GAP (alignment padding the parser skips over).
//! `cargo run --example diffclass -- <save> <steamid64>`

use std::{collections::BTreeSet, fs};

use anyhow::{Context, Result, bail};

const FIELD_TYPE_ARRAY: i32 = -1;
const FIELD_TYPE_STRING: i32 = 0x0f;
const FIELD_TYPE_CLASS: i32 = 0x11;
const ARRAY_TYPE_VALUE: i32 = 0;
const ARRAY_TYPE_CLASS: i32 = 1;
const ARRAY_MARKER: u32 = 0xffee_ffee;

struct Reader<'a> {
  data: &'a [u8],
  offset: usize,
  base: usize,
  consumed: BTreeSet<usize>,
}

impl<'a> Reader<'a> {
  fn new(data: &'a [u8]) -> Self {
    Self { data, offset: 0, base: 16, consumed: BTreeSet::new() }
  }

  fn remaining(&self) -> usize {
    self.data.len().saturating_sub(self.offset)
  }

  fn align(&mut self, alignment: usize) -> Result<()> {
    let absolute = self.base + self.offset;
    let aligned = absolute.div_ceil(alignment) * alignment;
    self.offset = aligned - self.base;
    if self.offset > self.data.len() {
      bail!("alignment exceeds payload bounds");
    }
    Ok(())
  }

  fn align_sized(&mut self, size: usize) -> Result<()> {
    self.align(size)
  }

  fn read_bytes(&mut self, len: usize) -> Result<Vec<u8>> {
    let end = self.offset + len;
    let value = self.data.get(self.offset..end).context("unexpected end")?.to_vec();
    for index in self.offset..end {
      self.consumed.insert(index);
    }
    self.offset = end;
    Ok(value)
  }

  fn read_u32(&mut self) -> Result<u32> {
    Ok(u32::from_le_bytes(self.read_bytes(4)?.try_into().expect("4 bytes")))
  }

  fn read_i32(&mut self) -> Result<i32> {
    Ok(i32::from_le_bytes(self.read_bytes(4)?.try_into().expect("4 bytes")))
  }

  fn peek_u32(&self) -> Option<u32> {
    Some(u32::from_le_bytes(self.data.get(self.offset..self.offset + 4)?.try_into().ok()?))
  }
}

/// Re-implements the payload parser, recording which byte offsets it consumed.
fn parse_consumed(data: &[u8]) -> Result<BTreeSet<usize>> {
  let mut reader = Reader::new(data);
  while reader.remaining() > 0 {
    if reader.remaining() < 12 && reader.rest_is_zero() {
      break;
    }
    let _native_hash = reader.read_u32()?;
    read_class(&mut reader)
      .with_context(|| format!("class stream failed at {:#x}", reader.offset))?;
  }
  Ok(reader.consumed)
}

impl<'a> Reader<'a> {
  fn rest_is_zero(&self) -> bool {
    self.data[self.offset..].iter().all(|byte| *byte == 0)
  }
}

fn read_class(reader: &mut Reader<'_>) -> Result<()> {
  let field_count = reader.read_u32()? as usize;
  let _hash = reader.read_u32()?;
  if field_count > reader.remaining() / 8 {
    bail!("impossible field count");
  }
  for _ in 0..field_count {
    read_field(reader)?;
  }
  Ok(())
}

fn read_field(reader: &mut Reader<'_>) -> Result<()> {
  let _hash = reader.read_u32()?;
  let field_type = reader.read_i32()?;
  match field_type {
    FIELD_TYPE_ARRAY => read_array(reader)?,
    FIELD_TYPE_CLASS => read_class(reader)?,
    FIELD_TYPE_STRING => {
      reader.align(4)?;
      let len = reader.read_u32()? as usize;
      if len > reader.remaining() / 2 {
        bail!("string too long");
      }
      reader.read_bytes(len * 2)?;
    }
    _ => {
      reader.align(4)?;
      let size = reader.read_u32()?;
      if size == 0 {
        bail!("zero-sized value");
      }
      if size != 1 {
        reader.align_sized(size as usize)?;
      }
      reader.read_bytes(size as usize)?;
    }
  }
  reader.align(4)?;
  Ok(())
}

fn read_array(reader: &mut Reader<'_>) -> Result<()> {
  reader.align(4)?;
  let member_type = reader.read_i32()?;
  let member_size = reader.read_u32()?;
  let len = reader.read_u32()? as usize;
  let array_type = reader.read_i32()?;
  if array_type != ARRAY_TYPE_VALUE && array_type != ARRAY_TYPE_CLASS {
    bail!("unsupported array type");
  }
  if array_type == ARRAY_TYPE_CLASS && reader.peek_u32() == Some(ARRAY_MARKER) {
    reader.read_u32()?;
    reader.read_bytes(len * 4)?;
  }
  for _ in 0..len {
    if array_type == ARRAY_TYPE_CLASS {
      read_class(reader)?;
    } else {
      match member_type {
        FIELD_TYPE_ARRAY => read_array(reader)?,
        FIELD_TYPE_STRING => {
          reader.align(4)?;
          let slen = reader.read_u32()? as usize;
          if slen > reader.remaining() / 2 {
            bail!("string too long");
          }
          reader.read_bytes(slen * 2)?;
        }
        _ => {
          if member_size != 1 {
            reader.align_sized(member_size as usize)?;
          }
          reader.read_bytes(member_size as usize)?;
        }
      }
    }
  }
  reader.align(4)?;
  Ok(())
}

fn main() -> Result<()> {
  let mut args = std::env::args().skip(1);
  let save = args.next().expect("save path");
  let steamid64: u64 = args.next().expect("steamid64").parse().expect("numeric");
  let _ = args.next();

  use mhrise_save_editor::container::SteamSave;
  const STEAM_CLASS_STREAM_OFFSET: usize = 16;
  let document = SteamSave::open_path(&save, steamid64)?;
  let original = document.original_decrypted().to_vec();
  let encoded = document.payload().encode_at_offset(STEAM_CLASS_STREAM_OFFSET)?;

  let original_data = parse_consumed(&original)
    .with_context(|| format!("could not map the original payload ({} bytes)", original.len()))?;
  let encoded_data = parse_consumed(&encoded)
    .with_context(|| format!("could not map the re-encoded payload ({} bytes)", encoded.len()))?;

  let common = original.len().min(encoded.len());
  let mut data_diffs = 0u64;
  let mut gap_diffs = 0u64;
  let mut first_data_diffs: Vec<usize> = Vec::new();
  for offset in 0..common {
    if original[offset] != encoded[offset] {
      // Classify by the ORIGINAL parse: was this byte consumed as data there?
      // (Both parses must agree the byte is a gap for the diff to be benign.)
      if original_data.contains(&offset) || encoded_data.contains(&offset) {
        data_diffs += 1;
        if first_data_diffs.len() < 10 {
          first_data_diffs.push(offset);
        }
      } else {
        gap_diffs += 1;
      }
    }
  }
  println!(
    "original consumed {} data bytes, re-encoded consumed {} data bytes",
    original_data.len(),
    encoded_data.len()
  );
  println!("diff bytes classified: {gap_diffs} in gaps, {data_diffs} in DATA");
  if let Some(last) = original_data.iter().next_back() {
    println!("original parse consumed up to {last:#x} of {:#x}", original.len());
  }
  if let Some(first) = first_data_diffs.first() {
    println!("FIRST DATA DIFFERENCES at: {first_data_diffs:#x?} (first at {first:#x})");
    for &offset in &first_data_diffs {
      let from = offset.saturating_sub(10);
      let to = (offset + 10).min(common);
      println!(
        "  @{offset:#x} original: {}",
        original[from..to].iter().map(|b| format!("{b:02x}")).collect::<String>()
      );
      println!(
        "            re-encoded: {}",
        encoded[from..to].iter().map(|b| format!("{b:02x}")).collect::<String>()
      );
    }
  }
  let _ = fs::metadata(&save)?;
  Ok(())
}
