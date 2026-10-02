//! Whole-directory backup and restore for save folders.
//!
//! A backup is a verbatim copy of every regular file under the source
//! directory (subdirectories included, unrelated files included — a safety
//! net must be complete). Every copy is re-read and byte-compared before the
//! command reports success.

use std::{
  fs,
  io::Read,
  path::{Path, PathBuf},
  time::SystemTime,
};

use anyhow::{Context, Result, bail};

#[derive(Debug, Clone)]
pub struct FileRecord {
  pub relative: PathBuf,
  pub bytes: u64,
}

#[derive(Debug, Clone)]
pub struct BackupReport {
  pub source: PathBuf,
  pub destination: PathBuf,
  pub files: Vec<FileRecord>,
}

impl BackupReport {
  pub fn total_bytes(&self) -> u64 {
    self.files.iter().map(|file| file.bytes).sum()
  }
}

#[derive(Debug, Clone)]
pub struct RestoreReport {
  pub backup: PathBuf,
  pub target: PathBuf,
  pub files: Vec<FileRecord>,
}

impl RestoreReport {
  pub fn total_bytes(&self) -> u64 {
    self.files.iter().map(|file| file.bytes).sum()
  }
}

/// Copies every file under `source` into `destination` (default: a timestamped
/// sibling directory named `<source>-backup-<YYYYMMDD-HHMMSS>`) and verifies
/// every copy byte-for-byte. The source is only ever read.
pub fn backup_dir(source: &Path, output: Option<&Path>) -> Result<BackupReport> {
  if !source.is_dir() {
    bail!("{} is not a directory", source.display());
  }
  let destination = match output {
    Some(path) => path.to_path_buf(),
    None => default_backup_destination(source),
  };
  if destination.starts_with(source) {
    bail!("backup destination {} must be outside the source directory", destination.display());
  }
  if destination.exists() {
    bail!("backup destination {} already exists", destination.display());
  }
  let files = collect_files(source)?;
  if files.is_empty() {
    bail!("{} contains no files to back up", source.display());
  }

  fs::create_dir_all(&destination)
    .with_context(|| format!("could not create {}", destination.display()))?;
  for (relative, absolute) in &files {
    let copied = destination.join(relative);
    if let Some(parent) = copied.parent() {
      fs::create_dir_all(parent)
        .with_context(|| format!("could not create {}", parent.display()))?;
    }
    fs::copy(absolute, &copied)
      .with_context(|| format!("could not copy {} to {}", absolute.display(), copied.display()))?;
  }
  verify_copies(&destination, &files)?;

  let records = files
    .iter()
    .map(|(relative, absolute)| FileRecord {
      relative: relative.clone(),
      bytes: fs::metadata(absolute).map(|meta| meta.len()).unwrap_or(0),
    })
    .collect();
  Ok(BackupReport { source: source.to_path_buf(), destination, files: records })
}

/// Copies a backup back into `target`. Existing target files are only
/// overwritten with `force`, so a wrong-directory restore fails loudly.
pub fn restore_dir(backup: &Path, target: &Path, force: bool) -> Result<RestoreReport> {
  if !backup.is_dir() {
    bail!("{} is not a backup directory", backup.display());
  }
  if target.starts_with(backup) || backup.starts_with(target) {
    bail!("backup and target must be distinct directories");
  }
  let files = collect_files(backup)?;
  if files.is_empty() {
    bail!("{} contains no files to restore", backup.display());
  }
  if !force {
    let existing = files.iter().filter(|(relative, _)| target.join(relative).exists()).count();
    if existing > 0 {
      bail!(
        "{existing} file(s) already exist in {}; pass --force to overwrite them",
        target.display()
      );
    }
  }

  fs::create_dir_all(target).with_context(|| format!("could not create {}", target.display()))?;
  for (relative, absolute) in &files {
    let restored = target.join(relative);
    if let Some(parent) = restored.parent() {
      fs::create_dir_all(parent)
        .with_context(|| format!("could not create {}", parent.display()))?;
    }
    fs::copy(absolute, &restored).with_context(|| {
      format!("could not copy {} to {}", absolute.display(), restored.display())
    })?;
  }
  verify_copies(target, &files)?;

  let records = files
    .iter()
    .map(|(relative, absolute)| FileRecord {
      relative: relative.clone(),
      bytes: fs::metadata(absolute).map(|meta| meta.len()).unwrap_or(0),
    })
    .collect();
  Ok(RestoreReport { backup: backup.to_path_buf(), target: target.to_path_buf(), files: records })
}

fn verify_copies(destination: &Path, files: &[(PathBuf, PathBuf)]) -> Result<()> {
  for (relative, absolute) in files {
    let copied = destination.join(relative);
    if !files_equal(absolute, &copied)? {
      bail!(
        "verification failed: {} and {} differ; remove the broken backup at {}",
        absolute.display(),
        copied.display(),
        destination.display()
      );
    }
  }
  Ok(())
}

fn collect_files(root: &Path) -> Result<Vec<(PathBuf, PathBuf)>> {
  let mut files = Vec::new();
  collect_recursive(root, root, &mut files)?;
  files.sort();
  Ok(files)
}

fn collect_recursive(root: &Path, dir: &Path, files: &mut Vec<(PathBuf, PathBuf)>) -> Result<()> {
  let entries = fs::read_dir(dir).with_context(|| format!("could not read {}", dir.display()))?;
  for entry in entries {
    let entry = entry.with_context(|| format!("could not read an entry in {}", dir.display()))?;
    let path = entry.path();
    let file_type = entry.file_type()?;
    if file_type.is_dir() {
      collect_recursive(root, &path, files)?;
    } else if file_type.is_file() {
      let relative = path.strip_prefix(root).expect("entry is below root").to_path_buf();
      files.push((relative, path));
    } else {
      bail!(
        "{} is neither a regular file nor a directory; refusing to back up blindly",
        path.display()
      );
    }
  }
  Ok(())
}

fn default_backup_destination(source: &Path) -> PathBuf {
  let suffix = timestamp_suffix();
  // Inside the real Steam layout (userdata/<account>/<appid>/remote/win64_save)
  // a sibling backup would land in the Steam Cloud sync scope, where it can be
  // uploaded or pruned; place it under Documents instead.
  if is_steam_save_dir(source)
    && let Some(destination) = documents_backup_destination(source, &suffix)
  {
    return destination;
  }
  let name = source.file_name().and_then(|name| name.to_str()).unwrap_or("save");
  let parent = source.parent().unwrap_or(Path::new("."));
  parent.join(format!("{name}-backup-{suffix}"))
}

fn is_steam_save_dir(source: &Path) -> bool {
  source.file_name().is_some_and(|name| name == "win64_save")
    && source.parent().and_then(|parent| parent.file_name()).is_some_and(|name| name == "remote")
}

fn documents_backup_destination(source: &Path, suffix: &str) -> Option<PathBuf> {
  // userdata/<account>/<appid>/remote/win64_save: the account folder sits
  // three levels above the save directory.
  let account = source.parent()?.parent()?.parent()?.file_name()?.to_str()?.to_owned();
  let home = std::env::var_os("USERPROFILE")?;
  Some(PathBuf::from(home).join("Documents").join("MHR-Save-Backups").join(account).join(suffix))
}

fn timestamp_suffix() -> String {
  match time::OffsetDateTime::now_local() {
    Ok(now) => format!(
      "{:04}{:02}{:02}-{:02}{:02}{:02}",
      now.year(),
      u8::from(now.month()),
      now.day(),
      now.hour(),
      now.minute(),
      now.second(),
    ),
    Err(_) => format!(
      "unix-{}",
      SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
    ),
  }
}

fn files_equal(first: &Path, second: &Path) -> Result<bool> {
  let meta_first = fs::metadata(first)?;
  let meta_second = fs::metadata(second)?;
  if meta_first.len() != meta_second.len() {
    return Ok(false);
  }

  let mut file_first = fs::File::open(first)?;
  let mut file_second = fs::File::open(second)?;
  let mut buffer_first = vec![0u8; 64 * 1024];
  let mut buffer_second = vec![0u8; 64 * 1024];
  loop {
    let read_first = file_first.read(&mut buffer_first)?;
    let read_second = file_second.read(&mut buffer_second)?;
    if read_first != read_second {
      return Ok(false);
    }
    if read_first == 0 {
      return Ok(true);
    }
    if buffer_first[..read_first] != buffer_second[..read_second] {
      return Ok(false);
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn write_tree(dir: &Path) {
    fs::create_dir_all(dir.join("sub")).expect("subdir");
    fs::write(dir.join("data001Slot.bin"), b"slot one").expect("root file");
    fs::write(dir.join("data00-1.bin"), b"system").expect("root file 2");
    fs::write(dir.join("sub").join("nested.bin"), b"nested").expect("nested file");
  }

  #[test]
  fn backup_copies_and_verifies_everything() {
    let temp = tempfile::tempdir().expect("tempdir");
    write_tree(temp.path());

    let report = backup_dir(temp.path(), None).expect("backup should succeed");
    assert_eq!(report.files.len(), 3);
    assert_eq!(report.total_bytes(), 20);
    for relative in ["data001Slot.bin", "data00-1.bin", "sub/nested.bin"] {
      let copied = report.destination.join(relative);
      assert!(copied.exists(), "{relative} should exist in the backup");
      assert!(files_equal(&temp.path().join(relative), &copied).expect("compare should work"));
    }
  }

  #[test]
  fn backup_refuses_existing_destination_and_reuses_names() {
    let temp = tempfile::tempdir().expect("tempdir");
    let destination_parent = tempfile::tempdir().expect("tempdir for destination");
    write_tree(temp.path());
    let destination = destination_parent.path().join("backup");

    backup_dir(temp.path(), Some(&destination)).expect("first backup should succeed");
    let error = backup_dir(temp.path(), Some(&destination))
      .expect_err("second backup to the same path should fail");
    assert!(error.to_string().contains("already exists"), "unexpected error: {error}");
  }

  #[test]
  fn restore_is_blocked_without_force_and_works_with_force() {
    let temp = tempfile::tempdir().expect("tempdir");
    write_tree(temp.path());
    let report = backup_dir(temp.path(), None).expect("backup should succeed");
    let target = tempfile::tempdir().expect("tempdir for target");

    restore_dir(&report.destination, target.path(), false).expect("fresh restore should work");

    // Corrupt the target, then restore again: without --force it must refuse,
    // with --force it must bring the original bytes back.
    fs::write(target.path().join("data001Slot.bin"), b"broken").expect("corrupt file");
    let error = restore_dir(&report.destination, target.path(), false)
      .expect_err("overwriting restore without --force should fail");
    assert!(error.to_string().contains("--force"), "unexpected error: {error}");

    restore_dir(&report.destination, target.path(), true).expect("forced restore should work");
    let restored = fs::read(target.path().join("data001Slot.bin")).expect("read restored file");
    assert_eq!(restored, b"slot one");
  }

  #[test]
  fn restore_refuses_to_target_inside_backup() {
    let temp = tempfile::tempdir().expect("tempdir");
    write_tree(temp.path());
    let report = backup_dir(temp.path(), None).expect("backup should succeed");

    let inside = report.destination.join("inner");
    let error = restore_dir(&report.destination, &inside, false)
      .expect_err("restore into the backup itself should fail");
    assert!(error.to_string().contains("distinct"), "unexpected error: {error}");
  }

  #[test]
  fn backup_refuses_non_directory() {
    let temp = tempfile::tempdir().expect("tempdir");
    let file = temp.path().join("not-a-dir.bin");
    fs::write(&file, b"x").expect("write file");
    let error = backup_dir(&file, None).expect_err("file source should fail");
    assert!(error.to_string().contains("not a directory"), "unexpected error: {error}");
  }
}
