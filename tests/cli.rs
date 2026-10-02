//! End-to-end CLI tests against synthetic Steam saves. Real-save validation
//! is a manual step documented in README.md.

use std::fs;

use assert_cmd::Command;
use mhrise_save_editor::{
  container::{SteamSave, pack_steam},
  payload::{Array, ArrayValue, Class, Field, FieldValue, NativeClass, SavePayload},
};

const TEST_STEAM_ID: u64 = 76_561_198_382_766_028;

fn sample_payload() -> SavePayload {
  SavePayload {
    entries: vec![NativeClass {
      native_hash: 0x1322_883a,
      class: Class {
        hash: 0x9ccc_3b1e,
        fields: vec![
          Field {
            hash: 0x0a96_0102,
            field_type: 0x08,
            value: FieldValue::Scalar { size: 4, bytes: 100u32.to_le_bytes().to_vec() },
          },
          Field {
            hash: 0xabcd_ef01,
            field_type: -1,
            value: FieldValue::Array(Array {
              member_type: 0x11,
              member_size: 0,
              array_type: 1,
              class_hashes: Some(vec![0xaaaa_0001, 0xaaaa_0001]),
              values: (0..2u32)
                .map(|index| {
                  ArrayValue::Class(Box::new(Class {
                    hash: 0xaaaa_0001,
                    fields: vec![
                      Field {
                        hash: 0xaaaa_0002,
                        field_type: 0x08,
                        value: FieldValue::Scalar {
                          size: 4,
                          bytes: (1000 + index).to_le_bytes().to_vec(),
                        },
                      },
                      Field {
                        hash: 0xaaaa_0003,
                        field_type: 0x07,
                        value: FieldValue::Scalar { size: 4, bytes: 2u32.to_le_bytes().to_vec() },
                      },
                    ],
                  }))
                })
                .collect(),
            }),
          },
        ],
      },
    }],
  }
}

fn write_fixture_save(dir: &std::path::Path) -> std::path::PathBuf {
  let payload = sample_payload();
  let decrypted = payload.encode_at_offset(16).expect("fixture payload should encode");
  let file = pack_steam(&decrypted, TEST_STEAM_ID, 0).expect("fixture save should pack");
  let path = dir.join("data001Slot.bin");
  fs::write(&path, file).expect("fixture save should write");
  path
}

fn cli() -> Command {
  Command::cargo_bin("mhrise-save-editor").expect("binary should exist")
}

#[test]
fn inspect_reports_container_facts() {
  let dir = tempfile::tempdir().expect("tempdir");
  let save = write_fixture_save(dir.path());

  cli()
    .args(["inspect", save.to_str().expect("utf8 path")])
    .assert()
    .success()
    .stdout(predicates::str::contains("platform=Steam"));
}

#[test]
fn dump_then_apply_roundtrips_the_payload() {
  let dir = tempfile::tempdir().expect("tempdir");
  let save = write_fixture_save(dir.path());
  let json_path = dir.path().join("dump.json");
  let edited_path = dir.path().join("edited.bin");

  cli()
    .args([
      "dump-json",
      save.to_str().expect("utf8 path"),
      "--steamid64",
      &TEST_STEAM_ID.to_string(),
      "-o",
      json_path.to_str().expect("utf8 path"),
    ])
    .assert()
    .success();

  cli()
    .args([
      "apply-json",
      save.to_str().expect("utf8 path"),
      json_path.to_str().expect("utf8 path"),
      "-o",
      edited_path.to_str().expect("utf8 path"),
    ])
    .assert()
    .success();

  let reopened = SteamSave::open_path(&edited_path, TEST_STEAM_ID).expect("edited save opens");
  assert_eq!(*reopened.payload(), sample_payload());
}

#[test]
fn apply_json_applies_edited_values() {
  let dir = tempfile::tempdir().expect("tempdir");
  let save = write_fixture_save(dir.path());
  let json_path = dir.path().join("dump.json");
  let edited_path = dir.path().join("edited.bin");

  cli()
    .args([
      "dump-json",
      save.to_str().expect("utf8 path"),
      "--steamid64",
      &TEST_STEAM_ID.to_string(),
      "-o",
      json_path.to_str().expect("utf8 path"),
    ])
    .assert()
    .success();

  let text = fs::read_to_string(&json_path).expect("dump should read");
  // hex is authoritative; the display value must be kept consistent.
  // 9999 = 0x270f -> little-endian "0f270000".
  let edited = text
    .replace("\"hex\": \"64000000\"", "\"hex\": \"0f270000\"")
    .replace("\"value\": \"100\"", "\"value\": \"9999\"");
  assert_ne!(text, edited, "fixture scalar should appear in the dump");
  fs::write(&json_path, edited).expect("edited dump should write");

  cli()
    .args([
      "apply-json",
      save.to_str().expect("utf8 path"),
      json_path.to_str().expect("utf8 path"),
      "--steamid64",
      &TEST_STEAM_ID.to_string(),
      "-o",
      edited_path.to_str().expect("utf8 path"),
    ])
    .assert()
    .success();

  let reopened = SteamSave::open_path(&edited_path, TEST_STEAM_ID).expect("edited save opens");
  let FieldValue::Scalar { bytes, .. } = &reopened.payload().entries[0].class.fields[0].value
  else {
    panic!("first field should be a scalar");
  };
  assert_eq!(bytes.as_slice(), 9999u32.to_le_bytes());
}

#[test]
fn repack_is_accepted_and_refuses_same_output() {
  let dir = tempfile::tempdir().expect("tempdir");
  let save = write_fixture_save(dir.path());
  let repacked_path = dir.path().join("data001Slot.repacked.bin");

  cli()
    .args([
      "repack",
      save.to_str().expect("utf8 path"),
      "--steamid64",
      &TEST_STEAM_ID.to_string(),
      "-o",
      repacked_path.to_str().expect("utf8 path"),
    ])
    .assert()
    .success();

  // Repacking onto the source file must be refused.
  cli()
    .args([
      "repack",
      save.to_str().expect("utf8 path"),
      "--steamid64",
      &TEST_STEAM_ID.to_string(),
      "-o",
      save.to_str().expect("utf8 path"),
    ])
    .assert()
    .failure();
}

#[test]
fn roundtrip_reports_identity_on_synthetic_saves() {
  let dir = tempfile::tempdir().expect("tempdir");
  let save = write_fixture_save(dir.path());

  cli()
    .args([
      "roundtrip",
      save.to_str().expect("utf8 path"),
      "--steamid64",
      &TEST_STEAM_ID.to_string(),
    ])
    .assert()
    .success()
    .stdout(predicates::str::contains("BYTE-IDENTICAL"));
}

#[test]
fn backup_then_restore_via_cli() {
  let dir = tempfile::tempdir().expect("tempdir");
  let source = dir.path().join("saves");
  fs::create_dir_all(&source).expect("source dir");
  write_fixture_save(&source);
  fs::write(source.join("data00-1.bin"), b"system file").expect("extra file");
  let backup_path = dir.path().join("backup");
  let target = dir.path().join("restored");

  cli()
    .args([
      "backup",
      source.to_str().expect("utf8 path"),
      "-o",
      backup_path.to_str().expect("utf8 path"),
    ])
    .assert()
    .success()
    .stdout(predicates::str::contains("Verified"));

  cli()
    .args([
      "restore",
      backup_path.to_str().expect("utf8 path"),
      target.to_str().expect("utf8 path"),
    ])
    .assert()
    .success();

  // Restoring over existing files must refuse without --force and succeed with it.
  cli()
    .args([
      "restore",
      backup_path.to_str().expect("utf8 path"),
      target.to_str().expect("utf8 path"),
    ])
    .assert()
    .failure();
  cli()
    .args([
      "restore",
      backup_path.to_str().expect("utf8 path"),
      target.to_str().expect("utf8 path"),
      "--force",
    ])
    .assert()
    .success();
}
