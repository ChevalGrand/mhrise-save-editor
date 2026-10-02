//! Debug helper: prints hex around the first differing bytes between the
//! decrypted payload of a save and its re-encode.
//! `cargo run --example hexdiff -- <save> <steamid64> [regions]`

use std::fmt::Write as _;

use mhrise_save_editor::{container::SteamSave, payload::SavePayload};

const STEAM_CLASS_STREAM_OFFSET: usize = 16;

fn hex_window(data: &[u8], center: usize, radius: usize) -> String {
  let mut start = center.saturating_sub(radius);
  let end = (center + radius).min(data.len());
  let mut text = String::new();
  for chunk in data[start..end].chunks(16) {
    let _ = write!(text, "  {start:06x}:");
    for (index, byte) in chunk.iter().enumerate() {
      if start + index == center {
        let _ = write!(text, " [{byte:02x}]");
      } else {
        let _ = write!(text, " {byte:02x}");
      }
    }
    let _ = writeln!(text);
    start += chunk.len();
  }
  text
}

fn main() -> anyhow::Result<()> {
  let mut args = std::env::args().skip(1);
  let save = args.next().expect("save path");
  let steamid64: u64 = args.next().expect("steamid64").parse().expect("numeric steamid64");
  let regions: usize = args.next().and_then(|value| value.parse().ok()).unwrap_or(8);

  let document = SteamSave::open_path(&save, steamid64)?;
  let original = document.original_decrypted();
  let encoded = document.payload().encode_at_offset(STEAM_CLASS_STREAM_OFFSET)?;
  println!("original {} bytes, re-encoded {} bytes", original.len(), encoded.len());

  let common = original.len().min(encoded.len());
  let diffs: Vec<usize> = (0..common).filter(|&index| original[index] != encoded[index]).collect();
  println!("{} differing bytes in common range, first {} regions:", diffs.len(), regions);

  // Group diffs into runs and report their alignment. If every run sits
  // strictly inside alignment padding (start % 4 != 0 or interior bytes of a
  // run that ends on a 4-boundary), the difference is stale gap bytes, not data.
  let mut runs: Vec<(usize, usize)> = Vec::new();
  for &offset in &diffs {
    match runs.last_mut() {
      Some((_, end)) if *end == offset => *end += 1,
      _ => runs.push((offset, offset + 1)),
    }
  }
  let aligned_runs: Vec<(usize, usize)> =
    runs.iter().copied().filter(|(start, _)| start % 4 == 0).collect();
  println!("{} diff runs; starting at 4-aligned offsets: {}", runs.len(), aligned_runs.len());
  let mut end_mod_8_counts = std::collections::BTreeMap::new();
  for (_, end) in &aligned_runs {
    *end_mod_8_counts.entry(end % 8).or_insert(0) += 1;
  }
  println!("aligned-run end % 8 distribution: {end_mod_8_counts:?}");
  for (start, end) in aligned_runs.iter().take(regions) {
    println!("--- aligned run {start:#x}..{end:#x} ({} bytes) ---", end - start);
    print!("original:\n{}", hex_window(original, *start, 14));
    print!("re-encoded:\n{}", hex_window(&encoded, *start, 14));
  }
  for &offset in diffs.iter().take(regions) {
    println!("--- difference at {offset:#x} ---");
    print!("original:\n{}", hex_window(original, offset, 12));
    print!("re-encoded:\n{}", hex_window(&encoded, offset, 12));
  }

  if let Some((start, end)) = runs.last() {
    println!("last run: {start:#x}..{end:#x}, file ends at {common:#x}");
  }

  // Where do re-encoded bytes exist beyond the original length?
  if encoded.len() > original.len() {
    println!("re-encoded is {} bytes longer", encoded.len() - original.len());
  }
  let _ = SavePayload::parse_at_offset(&encoded, STEAM_CLASS_STREAM_OFFSET)?;
  Ok(())
}
