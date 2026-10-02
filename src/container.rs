//! Document layer for Steam (Citrus) core saves: open, edit, write.
//!
//! A [`SteamSave`] owns the decrypted payload of one core save file
//! (`data001Slot.bin` … `data003Slot.bin`), remembers the Curve Index
//! recovered from the file, and can serialize itself back into a valid
//! DSSS/Citrus container with a fresh MurmurHash3 outer checksum.

use std::{fs, io::Cursor, path::Path};

use anyhow::{Context, Result, bail};
use murmur3::murmur3_32;

use crate::{
  crypto::Citrus,
  format::{ChecksumStatus, DsssHeader, Platform, SaveFlags, checksum_status, parse_header},
  payload::SavePayload,
};

const FILE_HASH_LEN: usize = 4;
const CITRUS_SIZE_FIELD_LEN: usize = 8;
/// Steam core saves place the class stream at the first 16-byte boundary.
const STEAM_CLASS_STREAM_OFFSET: usize = 16;
const OUTER_HASH_SEED: u32 = 0xffff_ffff;

#[derive(Debug, Clone)]
pub struct SteamSave {
  steamid64: u64,
  curve_index: usize,
  payload: SavePayload,
  original_decrypted: Vec<u8>,
}

impl SteamSave {
  /// Opens a Steam core save. The Curve Index is recovered from the file
  /// itself, so no external configuration is needed beyond the SteamID64.
  pub fn open_path(path: impl AsRef<Path>, steamid64: u64) -> Result<Self> {
    let path = path.as_ref();
    let data = fs::read(path).with_context(|| format!("could not read {}", path.display()))?;
    Self::open_bytes(&data, steamid64).with_context(|| format!("could not open {}", path.display()))
  }

  pub fn open_bytes(data: &[u8], steamid64: u64) -> Result<Self> {
    let header = parse_header(data).context("invalid DSSS header")?;
    ensure_steam_core(&header)?;
    if !matches!(checksum_status(data)?, ChecksumStatus::Valid) {
      bail!("outer checksum is invalid; refusing to open a corrupt or truncated save");
    }

    let (encrypted, declared_len) = citrus_blob(data)?;
    let curve_index = Citrus::new(steamid64, None)
      .brute_force_find_params(encrypted, declared_len)
      .context("could not determine the Citrus Curve Index; check the SteamID64")?
      .index as usize;
    let decrypted = Citrus::new(steamid64, Some(curve_index))
      .decrypt(encrypted, declared_len)
      .context("Citrus decryption failed; check the SteamID64")?;
    let payload = SavePayload::parse_at_offset(&decrypted, STEAM_CLASS_STREAM_OFFSET)
      .context("invalid class stream")?;

    Ok(Self { steamid64, curve_index, payload, original_decrypted: decrypted })
  }

  pub fn steamid64(&self) -> u64 {
    self.steamid64
  }

  pub fn curve_index(&self) -> usize {
    self.curve_index
  }

  /// The decrypted payload exactly as it came out of the source save.
  pub fn original_decrypted(&self) -> &[u8] {
    &self.original_decrypted
  }

  pub fn payload(&self) -> &SavePayload {
    &self.payload
  }

  pub fn payload_mut(&mut self) -> &mut SavePayload {
    &mut self.payload
  }

  pub fn set_payload(&mut self, payload: SavePayload) {
    self.payload = payload;
  }

  /// Re-encodes and re-encrypts the payload into a valid save file.
  ///
  /// Output shorter than the original decrypted payload is zero-padded back
  /// to that length, so an unedited document reproduces the original decrypted
  /// bytes; a growing edit simply produces a longer payload, which the Citrus
  /// container supports through its declared-length field.
  pub fn to_bytes(&self) -> Result<Vec<u8>> {
    let mut decrypted = self
      .payload
      .encode_at_offset(STEAM_CLASS_STREAM_OFFSET)
      .context("could not encode the class stream")?;
    if decrypted.len() < self.original_decrypted.len() {
      decrypted.resize(self.original_decrypted.len(), 0);
    }
    pack_steam(&decrypted, self.steamid64, self.curve_index)
  }

  /// Writes the repacked save to a new file. Callers must never pass the
  /// source path; the CLI layer enforces that.
  pub fn write_to(&self, path: impl AsRef<Path>) -> Result<()> {
    let path = path.as_ref();
    let bytes = self.to_bytes()?;
    fs::write(path, bytes).with_context(|| format!("could not write {}", path.display()))
  }
}

/// Packs raw decrypted payload bytes into a Steam DSSS/Citrus container.
pub fn pack_steam(decrypted: &[u8], steamid64: u64, curve_index: usize) -> Result<Vec<u8>> {
  let mut output = Vec::new();
  output.extend_from_slice(b"DSSS");
  output.extend_from_slice(&2u32.to_le_bytes());
  output.extend_from_slice(&SaveFlags::CITRUS.bits().to_le_bytes());
  output.resize(STEAM_CLASS_STREAM_OFFSET, 0);

  let encrypted = Citrus::new(steamid64, Some(curve_index))
    .encrypt(decrypted)
    .context("Citrus encryption failed; check the Curve Index")?;
  output.extend_from_slice(&encrypted);
  output.extend_from_slice(&(decrypted.len() as u64).to_le_bytes());

  output.resize(align_up(output.len(), 4), 0);
  let hash =
    murmur3_32(&mut Cursor::new(&output), OUTER_HASH_SEED).context("could not hash the file")?;
  output.extend_from_slice(&hash.to_le_bytes());
  Ok(output)
}

fn ensure_steam_core(header: &DsssHeader) -> Result<()> {
  match header.platform() {
    Platform::Steam => Ok(()),
    Platform::NintendoSwitch => {
      bail!("this is a Nintendo Switch (DEFLATE) save; only Steam saves are supported")
    }
    Platform::Auxiliary => bail!("this is an auxiliary (album/screenshot) file, not a core save"),
    Platform::Unknown => bail!("unknown DSSS flag combination 0x{:08x}", header.raw_flags),
  }
}

fn citrus_blob(data: &[u8]) -> Result<(&[u8], usize)> {
  if data.len() < STEAM_CLASS_STREAM_OFFSET + CITRUS_SIZE_FIELD_LEN + FILE_HASH_LEN {
    bail!("save is too small to contain a Citrus payload");
  }
  let size_offset = data.len() - FILE_HASH_LEN - CITRUS_SIZE_FIELD_LEN;
  let declared_len = read_u64(data, size_offset)? as usize;
  Ok((&data[STEAM_CLASS_STREAM_OFFSET..size_offset], declared_len))
}

fn read_u64(data: &[u8], offset: usize) -> Result<u64> {
  let bytes =
    data.get(offset..offset + 8).context("unexpected end of file while reading the size field")?;
  Ok(u64::from_le_bytes(bytes.try_into().expect("fixed-size slice")))
}

fn align_up(value: usize, alignment: usize) -> usize {
  value.div_ceil(alignment) * alignment
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::payload::{Array, ArrayValue, Class, Field, FieldValue, NativeClass};

  const TEST_STEAM_ID: u64 = 76_561_198_382_766_028;
  const TEST_CURVE: usize = 0;
  const FIELD_TYPE_ARRAY: i32 = -1;
  const FIELD_TYPE_STRING: i32 = 0x0f;
  const ARRAY_TYPE_VALUE: i32 = 0;
  const ARRAY_TYPE_CLASS: i32 = 1;

  fn sample_payload() -> SavePayload {
    SavePayload {
      entries: vec![
        NativeClass {
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
                hash: 0x1234_5678,
                field_type: FIELD_TYPE_STRING,
                value: FieldValue::String("Hunter".encode_utf16().collect()),
              },
            ],
          },
        },
        NativeClass {
          native_hash: 0x1322_883a,
          class: Class {
            hash: 0x9ccc_3b1e,
            fields: vec![Field {
              hash: 0xabcd_ef01,
              field_type: FIELD_TYPE_ARRAY,
              value: FieldValue::Array(Array {
                member_type: 0x11,
                member_size: 0,
                array_type: ARRAY_TYPE_CLASS,
                class_hashes: Some(vec![0x1111_2222, 0x3333_4444]),
                values: vec![
                  ArrayValue::Class(Box::new(Class {
                    hash: 0x1111_2222,
                    fields: vec![Field {
                      hash: 0x5555_6666,
                      field_type: 0x04,
                      value: FieldValue::Scalar { size: 1, bytes: vec![42] },
                    }],
                  })),
                  ArrayValue::Class(Box::new(Class {
                    hash: 0x3333_4444,
                    fields: vec![Field {
                      hash: 0x7777_8888,
                      field_type: FIELD_TYPE_ARRAY,
                      value: FieldValue::Array(Array {
                        member_type: 0x08,
                        member_size: 4,
                        array_type: ARRAY_TYPE_VALUE,
                        class_hashes: None,
                        values: (0..5u32)
                          .map(|i| ArrayValue::Scalar(i.to_le_bytes().to_vec()))
                          .collect(),
                      }),
                    }],
                  })),
                ],
              }),
            }],
          },
        },
      ],
    }
  }

  fn packed_save(payload: &SavePayload) -> Vec<u8> {
    let decrypted = payload.encode_at_offset(STEAM_CLASS_STREAM_OFFSET).expect("fixture encodes");
    pack_steam(&decrypted, TEST_STEAM_ID, TEST_CURVE).expect("fixture packs")
  }

  #[test]
  fn open_write_roundtrip_preserves_payload() {
    let payload = sample_payload();
    let file = packed_save(&payload);

    let doc = SteamSave::open_bytes(&file, TEST_STEAM_ID).expect("fixture save should open");
    assert_eq!(doc.curve_index(), TEST_CURVE);
    assert_eq!(doc.payload(), &payload);

    let rewritten = doc.to_bytes().expect("repack should succeed");
    let reopened = SteamSave::open_bytes(&rewritten, TEST_STEAM_ID).expect("repack should reopen");
    assert_eq!(reopened.payload(), &payload);
  }

  #[test]
  fn unedited_repack_reproduces_decrypted_bytes() {
    let file = packed_save(&sample_payload());
    let doc = SteamSave::open_bytes(&file, TEST_STEAM_ID).expect("fixture save should open");

    let repacked = doc.to_bytes().expect("repack should succeed");
    let reopened = SteamSave::open_bytes(&repacked, TEST_STEAM_ID).expect("repack should reopen");
    assert_eq!(reopened.original_decrypted(), doc.original_decrypted());
  }

  #[test]
  fn edited_scalar_survives_repack() {
    let payload = sample_payload();
    let file = packed_save(&payload);

    let mut doc = SteamSave::open_bytes(&file, TEST_STEAM_ID).expect("fixture save should open");
    let FieldValue::Scalar { size: 4, bytes } =
      &mut doc.payload_mut().entries[0].class.fields[0].value
    else {
      panic!("fixture field should be a 4-byte scalar");
    };
    *bytes = 9999u32.to_le_bytes().to_vec();

    let rewritten = doc.to_bytes().expect("repack should succeed");
    let reopened = SteamSave::open_bytes(&rewritten, TEST_STEAM_ID).expect("repack should reopen");
    assert_eq!(
      reopened.payload().entries[0].class.fields[0].value,
      FieldValue::Scalar { size: 4, bytes: 9999u32.to_le_bytes().to_vec() }
    );
    // Untouched parts stay intact.
    assert_eq!(reopened.payload().entries[1], sample_payload().entries[1]);
  }

  #[test]
  fn grown_payload_survives_repack() {
    let payload = sample_payload();
    let file = packed_save(&payload);

    let mut doc = SteamSave::open_bytes(&file, TEST_STEAM_ID).expect("fixture save should open");
    let FieldValue::Array(array) = &mut doc.payload_mut().entries[1].class.fields[0].value else {
      panic!("fixture field should be an array");
    };
    array.class_hashes.as_mut().expect("fixture array has class hashes").push(0x5555_5555);
    array.values.push(ArrayValue::Class(Box::new(Class {
      hash: 0x5555_5555,
      fields: vec![Field {
        hash: 0x6666_6666,
        field_type: 0x08,
        value: FieldValue::Scalar { size: 4, bytes: 7u32.to_le_bytes().to_vec() },
      }],
    })));

    let rewritten = doc.to_bytes().expect("growing repack should succeed");
    let reopened = SteamSave::open_bytes(&rewritten, TEST_STEAM_ID).expect("repack should reopen");
    assert_eq!(reopened.payload(), doc.payload());
  }

  #[test]
  fn rejects_invalid_outer_checksum() {
    let mut file = packed_save(&sample_payload());
    let last = file.len() - 1;
    file[last] ^= 0xff;

    let error =
      SteamSave::open_bytes(&file, TEST_STEAM_ID).expect_err("corrupt save should be rejected");
    assert!(error.to_string().contains("checksum"), "unexpected error: {error}");
  }

  #[test]
  fn rejects_non_steam_save() {
    let mut file = Vec::new();
    file.extend_from_slice(b"DSSS");
    file.extend_from_slice(&2u32.to_le_bytes());
    file.extend_from_slice(&SaveFlags::empty().bits().to_le_bytes());
    file.extend_from_slice(&[0u8; FILE_HASH_LEN]);

    let error =
      SteamSave::open_bytes(&file, TEST_STEAM_ID).expect_err("auxiliary save should be rejected");
    assert!(error.to_string().contains("auxiliary"), "unexpected error: {error}");
  }
}
