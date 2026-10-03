//! Diagnostic: dump every ProgressSaveData instance's rank fields.

use mhrise_save_editor::{container::SteamSave, edit, payload::FieldValue};

fn main() -> anyhow::Result<()> {
  let path = std::env::args().nth(1).expect("save path");
  let steamid64: u64 = std::env::args().nth(2).expect("steamid64").parse().expect("u64");
  let document = SteamSave::open_path(std::path::Path::new(&path), steamid64)?;

  let mut instances = Vec::new();
  edit::collect_classes_public(document.payload(), edit::PROGRESS_SAVE_CLASS, &mut instances);
  println!("ProgressSaveData instances: {}", instances.len());
  for (index, class) in instances.iter().enumerate() {
    println!("instance {index}: {} fields", class.fields.len());
    for field in &class.fields {
      if [
        edit::HUNTER_RANK,
        edit::HUNTER_RANK_POINT,
        edit::MASTER_RANK,
        edit::MASTER_RANK_POINT,
        edit::MYSTERY_RESEARCH_LEVEL,
        edit::MYSTERY_RESEARCH_POINT,
      ]
      .contains(&field.hash)
        && let FieldValue::Scalar { size, bytes } = &field.value
      {
        println!("  {:08x} size={size} bytes={:02x?} le32={}", field.hash, bytes,
          u32::from_le_bytes(bytes.as_slice().try_into().unwrap_or([0;4])));
      }
    }
  }
  println!("read_ranks: {:?}", edit::read_ranks(document.payload()));
  Ok(())
}
