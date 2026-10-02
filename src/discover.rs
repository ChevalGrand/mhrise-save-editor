use std::{
  fmt, fs,
  path::{Path, PathBuf},
};

use anyhow::{Context, Result};

use crate::format::{Platform, checksum_status, inspect_path};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveFileKind {
  Core,
  Auxiliary,
}

impl fmt::Display for SaveFileKind {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str(match self {
      Self::Core => "core",
      Self::Auxiliary => "auxiliary",
    })
  }
}

#[derive(Debug, Clone)]
pub struct SaveFile {
  pub path: PathBuf,
  pub platform: Platform,
  pub checksum: String,
  pub kind: SaveFileKind,
}

pub type CoreFile = SaveFile;

pub fn is_core_filename(name: &str) -> bool {
  if name == "data00-1.bin" {
    return true;
  }

  let Some(slot_number) =
    name.strip_prefix("data").and_then(|value| value.strip_suffix("Slot.bin"))
  else {
    return false;
  };
  slot_number.len() == 3 && slot_number.bytes().all(|byte| byte.is_ascii_digit())
}

pub fn is_auxiliary_filename(name: &str) -> bool {
  ["SS1_", "SS4_", "SS7_"]
    .iter()
    .any(|prefix| name.strip_prefix(prefix).is_some_and(is_core_filename))
}

pub fn discover_core_files(input: &Path) -> Result<Vec<CoreFile>> {
  discover_files(input, false)
}

pub fn discover_save_files(input: &Path) -> Result<Vec<SaveFile>> {
  discover_files(input, true)
}

fn discover_files(input: &Path, include_auxiliary: bool) -> Result<Vec<SaveFile>> {
  let paths = if input.is_file() {
    vec![input.to_path_buf()]
  } else {
    let mut paths = fs::read_dir(input)
      .with_context(|| format!("could not read save directory {}", input.display()))?
      .map(|entry| entry.map(|entry| entry.path()))
      .collect::<std::io::Result<Vec<_>>>()?;
    paths.retain(|path| {
      path.file_name().and_then(|name| name.to_str()).is_some_and(|name| {
        is_core_filename(name) || (include_auxiliary && is_auxiliary_filename(name))
      })
    });
    paths.sort();
    paths
  };

  if paths.is_empty() {
    anyhow::bail!("no core save files found in {}", input.display());
  }

  paths
    .into_iter()
    .map(|path| {
      let (header, _) =
        inspect_path(&path).with_context(|| format!("could not inspect {}", path.display()))?;
      let data = fs::read(&path)?;
      let checksum = checksum_status(&data)?.to_string();
      let kind = path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| is_auxiliary_filename(name))
        .map_or(SaveFileKind::Core, |_| SaveFileKind::Auxiliary);
      Ok(SaveFile { path, platform: header.platform(), checksum, kind })
    })
    .collect()
}

#[cfg(test)]
mod tests {
  use super::{is_auxiliary_filename, is_core_filename};

  #[test]
  fn finds_core_files_but_ignores_album_files() {
    assert!(is_core_filename("data00-1.bin"));
    assert!(is_core_filename("data001Slot.bin"));
    assert!(is_core_filename("data003Slot.bin"));
    assert!(!is_core_filename("SS1_data001Slot.bin"));
    assert!(!is_core_filename("data01Slot.bin"));
    assert!(!is_core_filename("data001Slot.dat"));
    assert!(is_auxiliary_filename("SS1_data001Slot.bin"));
    assert!(is_auxiliary_filename("SS4_data030Slot.bin"));
    assert!(is_auxiliary_filename("SS7_data070Slot.bin"));
    assert!(!is_auxiliary_filename("SS2_data001Slot.bin"));
    assert!(!is_auxiliary_filename("SS1_data001Slot.dat"));
  }
}
