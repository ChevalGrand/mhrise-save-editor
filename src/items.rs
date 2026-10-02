//! Item id → internal name lookup.
//!
//! `item_names.json` is generated from the game's
//! `snow.data.ContentsIdSystem.ItemId` enum (see assets/mhrise/); the names
//! are the game's internal identifiers (`I_Normal_0006`), not localized
//! display names.

use std::{collections::BTreeMap, sync::OnceLock};

const ITEM_NAMES_JSON: &str = include_str!("../assets/mhrise/item_names.json");

fn item_names() -> &'static BTreeMap<String, u32> {
  static NAMES: OnceLock<BTreeMap<String, u32>> = OnceLock::new();
  NAMES.get_or_init(|| {
    serde_json::from_str::<BTreeMap<String, u32>>(ITEM_NAMES_JSON).expect("embedded item names")
  })
}

/// Looks up the internal name of an item id (for example `I_Normal_0006`).
pub fn item_name(id: u32) -> Option<&'static str> {
  item_names().iter().find(|(_, value)| **value == id).map(|(name, _)| name.as_str())
}

/// All item names with their ids, sorted by id.
pub fn all_items() -> Vec<(&'static str, u32)> {
  let names = item_names();
  let mut items: Vec<(&'static str, u32)> =
    names.iter().map(|(name, id)| (name.as_str(), *id)).collect();
  items.sort_by_key(|(_, id)| *id);
  items
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
  fn all_items_are_sorted_and_populated() {
    let items = all_items();
    assert!(items.len() > 1000);
    assert!(items.windows(2).all(|pair| pair[0].1 <= pair[1].1));
  }
}
