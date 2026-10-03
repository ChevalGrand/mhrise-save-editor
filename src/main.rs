use std::{
  fs,
  path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use mhrise_save_editor::{
  archive::{backup_dir, restore_dir},
  container::SteamSave,
  discover::{self, discover_core_files, discover_save_files},
  edit,
  format::{DsssHeader, parse_header},
  json::JsonSave,
};

#[derive(Debug, Parser)]
#[command(
  name = "mhrise-save-editor",
  version,
  about = "Inspect, dump, edit, and repack Monster Hunter Rise Steam saves"
)]
struct Cli {
  #[command(subcommand)]
  command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
  /// Show container facts for a save file or a win64_save directory.
  Inspect {
    path: PathBuf,
    /// SteamID64 owning the saves; when given, core slots also report the
    /// hunter/master/anomaly-research ranks.
    #[arg(long)]
    steamid64: Option<u64>,
  },
  /// Decrypt a save and dump its class stream as JSON.
  DumpJson {
    /// A core save file, or a directory (uses its first slot).
    save: PathBuf,
    /// SteamID64 that owns the save.
    #[arg(long)]
    steamid64: u64,
    /// Output JSON path; prints to stdout when omitted.
    #[arg(short, long)]
    output: Option<PathBuf>,
  },
  /// Load a dumped JSON (optionally edited) into a new save file.
  ApplyJson {
    /// The source save the JSON was dumped from.
    save: PathBuf,
    /// The JSON dump, possibly edited.
    json: PathBuf,
    /// SteamID64; defaults to the value recorded in the JSON metadata.
    #[arg(long)]
    steamid64: Option<u64>,
    /// Output save path.
    #[arg(short, long)]
    output: Option<PathBuf>,
  },
  /// Repack a save without changing its payload.
  Repack {
    save: PathBuf,
    #[arg(long)]
    steamid64: u64,
    #[arg(short, long)]
    output: Option<PathBuf>,
  },
  /// Compare a re-encode of the decrypted payload against the original bytes.
  Roundtrip {
    save: PathBuf,
    #[arg(long)]
    steamid64: u64,
  },
  /// Back up an entire save directory (verbatim copy, byte-verified).
  Backup {
    /// The save directory, e.g. .../1446780/remote/win64_save
    dir: PathBuf,
    /// Backup destination; defaults to a timestamped sibling directory.
    #[arg(short, long)]
    output: Option<PathBuf>,
  },
  /// Restore a backup made by the backup command into a directory.
  Restore {
    /// The backup directory.
    backup: PathBuf,
    /// Directory to restore into.
    target: PathBuf,
    /// Overwrite files that already exist in the target directory.
    #[arg(long)]
    force: bool,
  },
  /// Structurally diff two core saves (for example a fresh vs a progressed
  /// character) and report per-field changes, aggregated by hash path.
  Diff {
    /// The "before" save.
    save_a: PathBuf,
    /// The "after" save.
    save_b: PathBuf,
    #[arg(long)]
    steamid64: u64,
    /// Max detailed changes printed after the aggregates.
    #[arg(long, default_value_t = 30)]
    limit: usize,
  },
  /// Set the current wallet amount of a save.
  SetMoney {
    save: PathBuf,
    #[arg(long)]
    steamid64: u64,
    /// The new wallet amount.
    #[arg(long)]
    value: u32,
    /// Also set the lifetime money-gained counter.
    #[arg(long)]
    total_added: Option<u32>,
    #[arg(short, long)]
    output: Option<PathBuf>,
  },
  /// Set the current Kamura/Steady point balance of a save.
  SetPoints {
    save: PathBuf,
    #[arg(long)]
    steamid64: u64,
    /// The new point balance.
    #[arg(long)]
    value: u32,
    #[arg(short, long)]
    output: Option<PathBuf>,
  },
  /// Replace the target save's item box with the source save's item box.
  TransferItems {
    /// The save that receives the items (for example a new character).
    target: PathBuf,
    /// The save donating its item box.
    source: PathBuf,
    /// SteamID64 owning both saves.
    #[arg(long)]
    steamid64: u64,
    #[arg(short, long)]
    output: Option<PathBuf>,
  },
  /// Replace the target save's whole equipment complex with the source's:
  /// equipment box (weapons/armor/talismans), loadout registers, hunter sets,
  /// worn equipment pack, and item pouch loadouts.
  TransferEquipment {
    /// The save that receives the equipment (for example a new character).
    target: PathBuf,
    /// The save donating its equipment.
    source: PathBuf,
    /// SteamID64 owning both saves.
    #[arg(long)]
    steamid64: u64,
    #[arg(short, long)]
    output: Option<PathBuf>,
  },
}

fn main() -> Result<()> {
  let cli = Cli::parse();
  match cli.command {
    Command::Inspect { path, steamid64 } => inspect(&path, steamid64),
    Command::DumpJson { save, steamid64, output } => dump_json(&save, steamid64, output.as_deref()),
    Command::ApplyJson { save, json, steamid64, output } => {
      apply_json(&save, &json, steamid64, output.as_deref())
    }
    Command::Repack { save, steamid64, output } => repack(&save, steamid64, output.as_deref()),
    Command::Roundtrip { save, steamid64 } => roundtrip(&save, steamid64),
    Command::Backup { dir, output } => backup(&dir, output.as_deref()),
    Command::Restore { backup, target, force } => restore(&backup, &target, force),
    Command::Diff { save_a, save_b, steamid64, limit } => diff(&save_a, &save_b, steamid64, limit),
    Command::SetMoney { save, steamid64, value, total_added, output } => {
      set_money(&save, steamid64, value, total_added, output.as_deref())
    }
    Command::SetPoints { save, steamid64, value, output } => {
      set_points(&save, steamid64, value, output.as_deref())
    }
    Command::TransferItems { target, source, steamid64, output } => {
      transfer_items(&target, &source, steamid64, output.as_deref())
    }
    Command::TransferEquipment { target, source, steamid64, output } => {
      transfer_equipment(&target, &source, steamid64, output.as_deref())
    }
  }
}

fn inspect(path: &Path, steamid64: Option<u64>) -> Result<()> {
  let files = discover_save_files(path)?;
  println!("Input: {}", path.display());
  println!("Save files: {}", files.len());
  for file in files {
    let data = fs::read(&file.path)?;
    let header = parse_header(&data)?;
    println!(
      "- {}: kind={}, platform={}, checksum={}",
      file.path.file_name().and_then(|name| name.to_str()).unwrap_or("<unknown>"),
      file.kind,
      file.platform,
      file.checksum
    );
    print_header_details(&header, data.len());
    if let Some(steamid64) = steamid64
      && file.kind == discover::SaveFileKind::Core
      && let Ok(document) = SteamSave::open_path(&file.path, steamid64)
      && let Some(ranks) = edit::read_ranks(document.payload())
    {
      println!(
        "  ranks: HR={} MR={} MysteryResearch={}",
        ranks.hunter, ranks.master, ranks.mystery_research
      );
    }
  }
  Ok(())
}

fn print_header_details(header: &DsssHeader, len: usize) {
  println!("  dsss_version={}, flags=0x{:08x}, size={len} bytes", header.version, header.raw_flags);
}

fn dump_json(save: &Path, steamid64: u64, output: Option<&Path>) -> Result<()> {
  let save_path = resolve_core_save(save)?;
  let document = SteamSave::open_path(&save_path, steamid64)?;
  println!("Opened {} (Curve Index: {}, detected)", save_path.display(), document.curve_index());

  let dump =
    JsonSave::from_payload(document.payload(), Some(steamid64), Some(document.curve_index()));
  let text = serde_json::to_string_pretty(&dump).context("could not serialize the JSON dump")?;

  match output {
    Some(output) => {
      fs::write(output, text).with_context(|| format!("could not write {}", output.display()))?;
      println!("Dumped payload to {}", output.display());
    }
    None => println!("{text}"),
  }
  Ok(())
}

fn apply_json(
  save: &Path,
  json: &Path,
  steamid64: Option<u64>,
  output: Option<&Path>,
) -> Result<()> {
  let save_path = resolve_core_save(save)?;
  let text =
    fs::read_to_string(json).with_context(|| format!("could not read {}", json.display()))?;
  let dump: JsonSave =
    serde_json::from_str(&text).with_context(|| format!("could not parse {}", json.display()))?;

  let steamid64 = steamid64
    .or_else(|| dump.steamid64.as_deref().and_then(|id| id.parse::<u64>().ok()))
    .context("no SteamID64 available: pass --steamid64 or use a dump with metadata")?;
  let mut document = SteamSave::open_path(&save_path, steamid64)?;
  println!("Opened {} (Curve Index: {}, detected)", save_path.display(), document.curve_index());

  let payload = dump.to_payload().context("the JSON dump is invalid")?;
  // Encode once as a structural validation before touching the file system.
  payload.encode().context("the JSON dump does not encode into a valid class stream")?;
  document.set_payload(payload);

  let output = output.map(PathBuf::from).unwrap_or_else(|| derived_output(&save_path, "edited"));
  ensure_distinct(&save_path, &output)?;
  document.write_to(&output)?;
  println!(
    "Wrote {} (SteamID64: {}, Curve Index: {})",
    output.display(),
    document.steamid64(),
    document.curve_index()
  );
  Ok(())
}

fn repack(save: &Path, steamid64: u64, output: Option<&Path>) -> Result<()> {
  let save_path = resolve_core_save(save)?;
  let document = SteamSave::open_path(&save_path, steamid64)?;
  println!("Opened {} (Curve Index: {}, detected)", save_path.display(), document.curve_index());

  let output = output.map(PathBuf::from).unwrap_or_else(|| derived_output(&save_path, "repacked"));
  ensure_distinct(&save_path, &output)?;
  document.write_to(&output)?;
  println!("Wrote {}", output.display());
  Ok(())
}

fn roundtrip(save: &Path, steamid64: u64) -> Result<()> {
  let save_path = resolve_core_save(save)?;
  let document = SteamSave::open_path(&save_path, steamid64)?;
  println!("Opened {} (Curve Index: {}, detected)", save_path.display(), document.curve_index());

  let original = document.original_decrypted();
  let encoded =
    document.payload().encode_at_offset(16).context("could not re-encode the class stream")?;
  let mut padded = encoded;
  if padded.len() < original.len() {
    padded.resize(original.len(), 0);
  }

  println!("Original decrypted: {} bytes", original.len());
  println!("Re-encoded:         {} bytes", padded.len());

  let structural = mhrise_save_editor::payload::SavePayload::parse_at_offset(&padded, 16)
    .is_ok_and(|reparsed| *document.payload() == reparsed);
  println!("Structural roundtrip: {}", if structural { "OK" } else { "MISMATCH" });

  if padded == original {
    println!("Byte roundtrip: BYTE-IDENTICAL");
  } else {
    let common = padded.len().min(original.len());
    let differing = (0..common).filter(|&index| padded[index] != original[index]).count();
    let first = (0..common).find(|&index| padded[index] != original[index]);
    println!("Byte roundtrip: DIFFERS ({differing} differing bytes in the common range)");
    if let Some(offset) = first {
      println!("First difference at offset {offset:#x} (after the 16-byte header)");
    }
    if padded.len() != original.len() {
      println!(
        "Length difference: re-encoded is {} bytes {}",
        (padded.len() as isize - original.len() as isize).abs(),
        if padded.len() > original.len() { "longer" } else { "shorter" }
      );
    }
    println!(
      "Note: differences inside zero padding are usually harmless; a repacked save is the real test."
    );
  }
  Ok(())
}

fn backup(dir: &Path, output: Option<&Path>) -> Result<()> {
  let report = backup_dir(dir, output)?;
  println!(
    "Backed up {} file(s), {} bytes, from {} to {}",
    report.files.len(),
    report.total_bytes(),
    report.source.display(),
    report.destination.display()
  );
  for file in &report.files {
    println!("  - {} ({})", file.relative.display(), format_bytes(file.bytes));
  }
  println!("Verified: every copy is byte-identical to its source.");
  Ok(())
}

fn restore(backup: &Path, target: &Path, force: bool) -> Result<()> {
  let report = restore_dir(backup, target, force)?;
  println!(
    "Restored {} file(s), {} bytes, from {} into {}",
    report.files.len(),
    report.total_bytes(),
    report.backup.display(),
    report.target.display()
  );
  println!("Verified: every restored file is byte-identical to the backup.");
  Ok(())
}

fn diff(save_a: &Path, save_b: &Path, steamid64: u64, limit: usize) -> Result<()> {
  let path_a = resolve_core_save(save_a)?;
  let path_b = resolve_core_save(save_b)?;
  let document_a = SteamSave::open_path(&path_a, steamid64)?;
  let document_b = SteamSave::open_path(&path_b, steamid64)?;
  println!("A: {} ({} bytes decrypted)", path_a.display(), document_a.original_decrypted().len());
  println!("B: {} ({} bytes decrypted)", path_b.display(), document_b.original_decrypted().len());

  let report =
    mhrise_save_editor::diff::diff_payloads(document_a.payload(), document_b.payload(), usize::MAX);
  println!(
    "Total changes: {} ({} value changes, {} additions/removals combined)",
    report.total_changes,
    report
      .changes
      .iter()
      .filter(|change| change.kind == mhrise_save_editor::diff::ChangeKind::ValueChanged)
      .count(),
    report
      .changes
      .iter()
      .filter(|change| change.kind != mhrise_save_editor::diff::ChangeKind::ValueChanged)
      .count()
  );

  let aggregates = mhrise_save_editor::diff::aggregate_changes(&report.changes);
  println!("Aggregated by field path (top {}):", aggregates.len().min(40));
  for group in aggregates.iter().take(40) {
    let mut parts = Vec::new();
    if group.changed > 0 {
      parts.push(format!("{} changed", group.changed));
    }
    if group.added > 0 {
      parts.push(format!("{} added", group.added));
    }
    if group.removed > 0 {
      parts.push(format!("{} removed", group.removed));
    }
    println!("  {:6}  {}", parts.join(", "), group.aggregate_path);
    for (before, after) in group.samples.iter().take(2) {
      println!(
        "             {} -> {}",
        before.as_deref().unwrap_or("<absent>"),
        after.as_deref().unwrap_or("<absent>")
      );
    }
  }

  println!("First {limit} detailed changes:");
  for change in report.changes.iter().take(limit) {
    match change.kind {
      mhrise_save_editor::diff::ChangeKind::ValueChanged => {
        println!(
          "  changed {} [{}]: {} -> {}",
          change.path,
          change.aggregate_path,
          change.before.as_deref().unwrap_or("<absent>"),
          change.after.as_deref().unwrap_or("<absent>")
        );
      }
      kind => {
        println!(
          "  {} {}: {} -> {}",
          kind.label(),
          change.path,
          change.before.as_deref().unwrap_or("<absent>"),
          change.after.as_deref().unwrap_or("<absent>")
        );
      }
    }
  }
  Ok(())
}

fn set_money(
  save: &Path,
  steamid64: u64,
  value: u32,
  total_added: Option<u32>,
  output: Option<&Path>,
) -> Result<()> {
  let save_path = resolve_core_save(save)?;
  let mut document = SteamSave::open_path(&save_path, steamid64)?;
  println!("Opened {} (Curve Index: {}, detected)", save_path.display(), document.curve_index());

  let report = edit::set_money(document.payload_mut(), value, total_added)?;
  println!("Set {} x{}: {:?} -> {}", report.field, report.updated, report.before, report.after);

  let output = output.map(PathBuf::from).unwrap_or_else(|| derived_output(&save_path, "edited"));
  ensure_distinct(&save_path, &output)?;
  document.write_to(&output)?;
  println!("Wrote {}", output.display());
  Ok(())
}

fn set_points(save: &Path, steamid64: u64, value: u32, output: Option<&Path>) -> Result<()> {
  let save_path = resolve_core_save(save)?;
  let mut document = SteamSave::open_path(&save_path, steamid64)?;
  println!("Opened {} (Curve Index: {}, detected)", save_path.display(), document.curve_index());

  let report = edit::set_village_points(document.payload_mut(), value)?;
  println!("Set {} x{}: {:?} -> {}", report.field, report.updated, report.before, report.after);

  let output = output.map(PathBuf::from).unwrap_or_else(|| derived_output(&save_path, "edited"));
  ensure_distinct(&save_path, &output)?;
  document.write_to(&output)?;
  println!("Wrote {}", output.display());
  Ok(())
}

fn transfer_items(
  target: &Path,
  source: &Path,
  steamid64: u64,
  output: Option<&Path>,
) -> Result<()> {
  let target_path = resolve_core_save(target)?;
  let source_path = resolve_core_save(source)?;
  let mut target_document = SteamSave::open_path(&target_path, steamid64)?;
  let source_document = SteamSave::open_path(&source_path, steamid64)?;
  println!("Target: {} | Source: {}", target_path.display(), source_path.display());

  let report = edit::transfer_item_box(target_document.payload_mut(), source_document.payload())?;
  println!("Transferred item box: {} slots, {} with items", report.slots, report.items);

  let output =
    output.map(PathBuf::from).unwrap_or_else(|| derived_output(&target_path, "transferred"));
  ensure_distinct(&target_path, &output)?;
  target_document.write_to(&output)?;
  println!("Wrote {}", output.display());
  Ok(())
}

fn transfer_equipment(
  target: &Path,
  source: &Path,
  steamid64: u64,
  output: Option<&Path>,
) -> Result<()> {
  let target_path = resolve_core_save(target)?;
  let source_path = resolve_core_save(source)?;
  let mut target_document = SteamSave::open_path(&target_path, steamid64)?;
  let source_document = SteamSave::open_path(&source_path, steamid64)?;
  println!("Target: {} | Source: {}", target_path.display(), source_path.display());

  let report = edit::transfer_equipment(target_document.payload_mut(), source_document.payload())?;
  for (label, fields) in &report.classes {
    println!("Copied {label}: {fields} fields");
  }
  println!(
    "Equipment box: {} slots, {} with equipment (weapons/armor/talismans)",
    report.box_slots, report.box_used
  );

  let output =
    output.map(PathBuf::from).unwrap_or_else(|| derived_output(&target_path, "transferred"));
  ensure_distinct(&target_path, &output)?;
  target_document.write_to(&output)?;
  println!("Wrote {}", output.display());
  Ok(())
}

fn format_bytes(bytes: u64) -> String {
  if bytes >= 1024 * 1024 {
    format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
  } else if bytes >= 1024 {
    format!("{:.1} KiB", bytes as f64 / 1024.0)
  } else {
    format!("{bytes} B")
  }
}

/// Accepts a single save file, or a directory whose slot files are searched.
fn resolve_core_save(path: &Path) -> Result<PathBuf> {
  if path.is_file() {
    return Ok(path.to_path_buf());
  }
  for name in ["data001Slot.bin", "data002Slot.bin", "data003Slot.bin"] {
    let candidate = path.join(name);
    if candidate.is_file() {
      return Ok(candidate);
    }
  }
  let files = discover_core_files(path)?;
  files
    .first()
    .map(|file| file.path.clone())
    .context("no core save file found; pass a data###Slot.bin file directly")
}

fn derived_output(source: &Path, suffix: &str) -> PathBuf {
  let mut name = source.file_stem().and_then(|stem| stem.to_str()).unwrap_or("save").to_owned();
  name.push('.');
  name.push_str(suffix);
  name.push_str(".bin");
  source.with_file_name(name)
}

fn ensure_distinct(input: &Path, output: &Path) -> Result<()> {
  if input == output {
    bail!(
      "output {} must be different from the input; the source save is never modified",
      output.display()
    );
  }
  if input.exists() && output.exists() {
    let same = fs::canonicalize(input).ok() == fs::canonicalize(output).ok();
    if same {
      bail!(
        "output {} must be different from the input; the source save is never modified",
        output.display()
      );
    }
  }
  Ok(())
}
