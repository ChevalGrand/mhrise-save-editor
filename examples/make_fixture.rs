//! Generates a synthetic Steam save for exercising the CLI without a real
//! game save: `cargo run --example make_fixture -- <output-dir>`

use std::{fs, path::PathBuf};

use mhrise_save_editor::{
  container::pack_steam,
  payload::{Array, ArrayValue, Class, Field, FieldValue, NativeClass, SavePayload},
};

const DEMO_STEAM_ID: u64 = 76_561_198_382_766_028;

fn main() -> anyhow::Result<()> {
  let output = std::env::args().nth(1).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
  let payload = SavePayload {
    entries: vec![
      NativeClass {
        native_hash: 0x85e9_04c1,
        class: Class {
          hash: 0x5766_f30b,
          fields: vec![
            Field {
              hash: 0x0a96_0102,
              field_type: 0x08,
              value: FieldValue::Scalar { size: 4, bytes: 100u32.to_le_bytes().to_vec() },
            },
            Field {
              hash: 0x1234_5678,
              field_type: 0x0f,
              value: FieldValue::String("Demo Hunter".encode_utf16().collect()),
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
            field_type: -1,
            value: FieldValue::Array(Array {
              member_type: 0x11,
              member_size: 0,
              array_type: 1,
              class_hashes: Some(vec![0xaaaa_0001; 3]),
              values: (0..3u32)
                .map(|index| {
                  ArrayValue::Class(Box::new(Class {
                    hash: 0xaaaa_0001,
                    fields: vec![
                      Field {
                        hash: 0xaaaa_0002,
                        field_type: 0x08,
                        value: FieldValue::Scalar {
                          size: 4,
                          bytes: (3000 + index * 10).to_le_bytes().to_vec(),
                        },
                      },
                      Field {
                        hash: 0xaaaa_0003,
                        field_type: 0x07,
                        value: FieldValue::Scalar { size: 4, bytes: 5u32.to_le_bytes().to_vec() },
                      },
                    ],
                  }))
                })
                .collect(),
            }),
          }],
        },
      },
    ],
  };

  let decrypted = payload.encode_at_offset(16)?;
  let file = pack_steam(&decrypted, DEMO_STEAM_ID, 0)?;
  let path = output.join("data001Slot.bin");
  fs::write(&path, file)?;
  println!("wrote {} (SteamID64: {})", path.display(), DEMO_STEAM_ID);
  Ok(())
}
