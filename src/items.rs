//! Item id → display name lookup.
//!
//! `item_names.json` is generated from the game's
//! `snow.data.ContentsIdSystem.ItemId` enum (see assets/mhrise/); the names
//! are the game's internal identifiers (`I_Normal_0006`), not localized
//! display names. `item_names_cn.json` is the community 16.0.0 item table
//! keyed by the same numeric id and carries Chinese display names plus the
//! item category (`Consume`, `Material`, `Bullet`, …). Both are parsed once
//! and leaked so lookups can return `&'static str`.

use std::{
  collections::{BTreeMap, HashMap},
  sync::OnceLock,
};

const ITEM_NAMES_JSON: &str = include_str!("../assets/mhrise/item_names.json");
const ITEM_NAMES_CN_JSON: &str = include_str!("../assets/mhrise/item_names_cn.json");

/// Chinese display name and category keyed by numeric item id.
fn chinese_table() -> &'static HashMap<u32, (&'static str, &'static str)> {
  static TABLE: OnceLock<HashMap<u32, (&'static str, &'static str)>> = OnceLock::new();
  TABLE.get_or_init(|| {
    let parsed: BTreeMap<String, (String, String)> =
      serde_json::from_str(ITEM_NAMES_CN_JSON).expect("embedded chinese item names");
    let mut table = HashMap::with_capacity(parsed.len());
    for (key, (name, category)) in parsed {
      let id: u32 = key.parse().expect("decimal item id");
      let name: &'static str = Box::leak(name.into_boxed_str());
      let category: &'static str = Box::leak(category.into_boxed_str());
      table.insert(id, (name, category));
    }
    table
  })
}

/// Internal game identifier keyed by numeric item id.
fn internal_table() -> &'static HashMap<u32, &'static str> {
  static TABLE: OnceLock<HashMap<u32, &'static str>> = OnceLock::new();
  TABLE.get_or_init(|| {
    let parsed: BTreeMap<String, u32> =
      serde_json::from_str(ITEM_NAMES_JSON).expect("embedded item names");
    let mut table: HashMap<u32, &'static str> = HashMap::with_capacity(parsed.len());
    for (name, id) in parsed {
      let name: &'static str = Box::leak(name.into_boxed_str());
      table.entry(id).or_insert(name);
    }
    table
  })
}

/// The game's internal identifier of an item (for example `I_Normal_0006`).
pub fn item_name(id: u32) -> Option<&'static str> {
  internal_table().get(&id).copied()
}

/// The Chinese display name of an item (for example `回复药`), if the
/// community table knows it.
pub fn item_name_cn(id: u32) -> Option<&'static str> {
  chinese_table().get(&id).map(|(name, _)| *name)
}

/// The item category (`Consume`, `Material`, `Bullet`, …), if known.
pub fn item_category(id: u32) -> Option<&'static str> {
  chinese_table().get(&id).map(|(_, category)| *category)
}

/// Best-effort display name: Chinese name if known, else the internal
/// identifier, else the raw id in hex.
pub fn item_display_name(id: u32) -> String {
  item_name_cn(id)
    .or_else(|| item_name(id))
    .map(str::to_owned)
    .unwrap_or_else(|| format!("0x{id:08X}"))
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn resolves_known_item_ids() {
    assert_eq!(item_name(0x0410_0006), Some("I_Normal_0006"));
    assert_eq!(item_name(0x0400_0000), Some("I_Unclassified_None"));
    assert_eq!(item_name(0xdead_beef), None);
  }

  #[test]
  fn resolves_chinese_names_and_categories() {
    assert_eq!(item_name_cn(0x0410_0006), Some("回复药"));
    assert_eq!(item_category(0x0410_0006), Some("Consume"));
    // The table only covers real in-game items; the None sentinel has none.
    assert_eq!(item_name_cn(0x0400_0000), None);
    assert_eq!(item_category(0x0410_0007), Some("Consume")); // 回复药G
    assert_eq!(item_name_cn(0xdead_beef), None);
  }

  #[test]
  fn display_name_prefers_chinese_then_internal_then_hex() {
    assert_eq!(item_display_name(0x0410_0006), "回复药");
    assert_eq!(item_display_name(0x0400_0000), "I_Unclassified_None");
    assert_eq!(item_display_name(0xdead_beef), "0xDEADBEEF");
  }

  #[test]
  fn chinese_table_is_substantial_and_within_enum() {
    let cn = chinese_table();
    assert!(cn.len() > 1500, "chinese table too small: {}", cn.len());
    let internal = internal_table();
    assert!(cn.keys().all(|id| internal.contains_key(id)));
  }
}
