//! Targeted value editing for known gameplay fields.
//!
//! Field and class hashes are murmur3_32(name, seed 0xffffffff) over the RSZ
//! names (see README's schema section). Matching is constrained to both the
//! class hash and the field hash so that same-named fields in other classes
//! cannot be touched by accident.

use anyhow::{Result, bail};

use crate::payload::{Array, ArrayValue, Class, Field, FieldValue, SavePayload};

/// `snow.data.HandMoney`
pub const HAND_MONEY_CLASS: u32 = 0x0797_13ac;
/// `snow.data.HandMoney._Value` — the current wallet amount.
pub const HAND_MONEY_VALUE: u32 = 0x861a_b707;
/// `snow.data.HandMoney._TotalAddedValue` — lifetime money gained.
pub const HAND_MONEY_TOTAL_ADDED: u32 = 0x49dd_7535;
/// `snow.data.VillagePoint`
pub const VILLAGE_POINT_CLASS: u32 = 0x3067_35ee;
/// `snow.data.VillagePoint._Point` — the current Kamura/Steady point balance.
pub const VILLAGE_POINT_VALUE: u32 = 0xb280_aab0;
/// `snow.data.ItemBox`
pub const ITEM_BOX_CLASS: u32 = 0x6769_6161;
/// `snow.data.ItemBox._InventoryList`
pub const ITEM_BOX_INVENTORY_LIST: u32 = 0x7996_e7ee;
/// `snow.data.ItemInventoryData._ItemCount`
pub const ITEM_INVENTORY_COUNT: u32 = 0xc2b3_951c;
/// `snow.data.ItemCount._Num`
pub const ITEM_COUNT_NUM: u32 = 0x8c11_a916;
/// `snow.data.ItemCount._Id`
pub const ITEM_COUNT_ID: u32 = 0xaf48_5d0f;
/// `snow.data.ContentsIdSystem.ItemId.I_Unclassified_None` — the sentinel id
/// the game writes into empty item box slots (`_Num == 0`).
pub const ITEM_ID_NONE: u32 = 0x0400_0000;
/// `snow.data.EquipBox` — weapon/armor/talisman storage.
pub const EQUIP_BOX_CLASS: u32 = 0xf050_9899;
/// `snow.data.EquipDataManager.SaveData1` — worn pack, loadouts, hunter sets.
pub const EQUIP_MANAGER_SAVE_CLASS: u32 = 0x960b_aed3;
/// `EquipDataManager.SaveData1._PlEquipMySetList` — the 224 装备组合.
pub const EQUIP_MY_SET_LIST: u32 = 0xb4ab_1a25;
/// `PlEquipMySetData._IsUsing`.
pub const EQUIP_MYSET_IS_USING: u32 = 0x26fc_8108;
/// `PlEquipMySetData._Name`.
pub const EQUIP_MYSET_NAME: u32 = 0xbcf6_bc33;
/// `PlEquipMySetData._InventoryIndexList` — the equipment box indices the
/// loadout references (weapon + armor + talisman).
pub const EQUIP_MYSET_INVENTORY_INDEX_LIST: u32 = 0x466e_52be;
/// `snow.data.ItemMySet` — registered item pouch loadouts.
pub const ITEM_MY_SET_CLASS: u32 = 0x8923_cd68;
/// `snow.data.EquipmentInventoryData._IdVal` — the equipment piece id.
pub const EQUIP_ID_VAL: u32 = 0x6e86_4182;
/// `snow.data.EquipBox._WeaponArmorInventoryList`
pub const EQUIP_BOX_WEAPON_ARMOR_LIST: u32 = 0xa097_349e;
/// `snow.progress.ProgressSaveData` — the authoritative rank progression.
pub const PROGRESS_SAVE_CLASS: u32 = 0x948b_7b9d;
/// `snow.data.GuildCardData` — the self card carries display copies of the
/// ranks; buddy cards live in an array and must never be touched.
pub const GUILD_CARD_CLASS: u32 = 0x4454_1321;
/// `ProgressSaveData._HunterRank` / `GuildCardData.HunterRank`.
pub const HUNTER_RANK: u32 = 0x5653_c091;
/// `ProgressSaveData._HunterRankPoint` (accumulation only; never rewritten).
pub const HUNTER_RANK_POINT: u32 = 0x8789_d225;
/// `ProgressSaveData._MasterRank` / `GuildCardData._MasterRank`.
pub const MASTER_RANK: u32 = 0x4e42_8685;
/// `ProgressSaveData._MasterRankPoint`.
pub const MASTER_RANK_POINT: u32 = 0xdf24_c36f;
/// `ProgressSaveData._MysteryResearchLevel` — the 怪异研究等级.
pub const MYSTERY_RESEARCH_LEVEL: u32 = 0x6d6b_56e7;
/// `ProgressSaveData._MysteryResearchPoint`.
pub const MYSTERY_RESEARCH_POINT: u32 = 0x71a2_e552;
/// `GuildCardData.LaboLv` — the self card's copy of the 怪异研究等级.
pub const GUILD_CARD_LABO_LV: u32 = 0x665f_00e3;

/// `GuildCardData` self-card unlock flags (bool scalars, byte-sized).
pub const GUILD_FLAG_IS_OTOMO_SECRET_UNLOCKED: u32 = 0x4419_1620;
pub const GUILD_FLAG_IS_MR_RELEASE: u32 = 0xba45_9644;
pub const GUILD_FLAG_IS_MR_DISP: u32 = 0x4555_11ab;
pub const GUILD_FLAG_IS_SERVANT_QUEST_UNLOCKED: u32 = 0x8547_337b;
pub const GUILD_FLAG_IS_MYSTERY_QUEST_UNLOCKED: u32 = 0x00be_17fd;
pub const GUILD_FLAG_IS_END_CONTENTS_UNLOCKED: u32 = 0xcd20_735c;
pub const GUILD_FLAG_IS_CUSTOM_WEAPON_EDIT_UNLOCKED: u32 = 0x75e8_dceb;
pub const GUILD_FLAG_IS_SPECIAL_MYSTERY_UNLOCKED: u32 = 0x158c_02bf;

/// (field hash, human label) of the guild card unlock flags, in display
/// order for the GUI.
pub const GUILD_FLAGS: [(u32, &str); 8] = [
  (GUILD_FLAG_IS_MR_RELEASE, "大师等级解禁 (isMRRelease)"),
  (GUILD_FLAG_IS_MR_DISP, "显示大师等级 (isMRDisp)"),
  (GUILD_FLAG_IS_MYSTERY_QUEST_UNLOCKED, "怪异调查任务解锁"),
  (GUILD_FLAG_IS_SPECIAL_MYSTERY_UNLOCKED, "特别怪异调查解锁"),
  (GUILD_FLAG_IS_END_CONTENTS_UNLOCKED, "终盘内容解锁"),
  (GUILD_FLAG_IS_SERVANT_QUEST_UNLOCKED, "随从任务解锁"),
  (GUILD_FLAG_IS_OTOMO_SECRET_UNLOCKED, "随从秘密解锁"),
  (GUILD_FLAG_IS_CUSTOM_WEAPON_EDIT_UNLOCKED, "自定义武器编辑解锁"),
];

/// Reads the self guild card's unlock flags as (hash, value) pairs. Buddy
/// cards (nested in arrays) are never consulted.
pub fn read_guild_flags(payload: &SavePayload) -> Option<Vec<(u32, bool)>> {
  fn read(class: &Class, out: &mut Option<Vec<(u32, bool)>>) {
    if out.is_some() {
      return;
    }
    if class.hash == GUILD_CARD_CLASS {
      let mut flags = Vec::new();
      for (hash, _) in GUILD_FLAGS {
        let value = class.fields.iter().find(|field| field.hash == hash).and_then(|field| {
          match &field.value {
            FieldValue::Scalar { bytes, size: 1 } if bytes.len() == 1 => Some(bytes[0] != 0),
            _ => None,
          }
        });
        match value {
          Some(value) => flags.push((hash, value)),
          // Missing flag: schema drift; report the card as unreadable.
          None => return,
        }
      }
      *out = Some(flags);
      return;
    }
    for field in &class.fields {
      if let FieldValue::Class(nested) = &field.value {
        read(nested, out);
        if out.is_some() {
          return;
        }
      }
    }
  }
  let mut out = None;
  for entry in &payload.entries {
    read(&entry.class, &mut out);
    if out.is_some() {
      break;
    }
  }
  out
}

/// Writes the self guild card's unlock flags. Only byte-sized bool scalars
/// for the listed hashes are touched; buddy cards are never reached.
pub fn set_guild_flags(payload: &mut SavePayload, values: &[(u32, bool)]) -> Result<usize> {
  fn update(class: &mut Class, values: &[(u32, bool)], updated: &mut usize) {
    if class.hash == GUILD_CARD_CLASS {
      for field in &mut class.fields {
        if let Some(&(_, flag)) = values.iter().find(|(hash, _)| *hash == field.hash)
          && let FieldValue::Scalar { size: 1, bytes } = &mut field.value
          && bytes.len() == 1
        {
          *bytes = vec![u8::from(flag)];
          *updated += 1;
        }
      }
    }
    for field in &mut class.fields {
      if let FieldValue::Class(nested) = &mut field.value {
        update(nested, values, updated);
      }
    }
  }
  let mut updated = 0usize;
  for entry in &mut payload.entries {
    update(&mut entry.class, values, &mut updated);
  }
  if updated == 0 {
    bail!("self guild card not found; refusing to write unlock flags");
  }
  Ok(updated)
}

/// One 装备组合 register from `_PlEquipMySetList`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EquipLoadout {
  /// Slot number in the 224-entry list.
  pub index: usize,
  pub name: String,
  pub is_using: bool,
  /// Equipment box indices referenced by the loadout (`_InventoryIndexList`).
  pub inventory_indices: Vec<u32>,
}

/// Reads all 224 装备组合 registers from the target's EquipDataManager.
pub fn read_equip_loadouts(payload: &SavePayload) -> Vec<EquipLoadout> {
  let mut managers = Vec::new();
  collect_classes(payload, EQUIP_MANAGER_SAVE_CLASS, &mut managers);
  let Some(manager) = managers.first() else {
    return Vec::new();
  };
  let Some(FieldValue::Array(list)) =
    manager.fields.iter().find(|field| field.hash == EQUIP_MY_SET_LIST).map(|field| &field.value)
  else {
    return Vec::new();
  };
  list
    .values
    .iter()
    .enumerate()
    .filter_map(|(index, element)| {
      let ArrayValue::Class(class) = element else {
        return None;
      };
      let name = class.fields.iter().find(|field| field.hash == EQUIP_MYSET_NAME).and_then(
        |field| match &field.value {
          FieldValue::String(units) => Some(String::from_utf16_lossy(units)),
          _ => None,
        },
      )?;
      let is_using = class
        .fields
        .iter()
        .find(|field| field.hash == EQUIP_MYSET_IS_USING)
        .and_then(|field| match &field.value {
          FieldValue::Scalar { bytes, size: 1 } if !bytes.is_empty() => Some(bytes[0] != 0),
          _ => None,
        })
        .unwrap_or(false);
      let mut inventory_indices = Vec::new();
      if let Some(field) = class.fields.iter().find(|field| field.hash == EQUIP_MYSET_INVENTORY_INDEX_LIST)
        && let FieldValue::Array(indices) = &field.value
      {
        for element in &indices.values {
          if let ArrayValue::Scalar(bytes) = element
            && bytes.len() == 4
          {
            inventory_indices.push(i32::from_le_bytes(bytes.as_slice().try_into().expect("4 bytes")) as u32);
          }
        }
      }
      Some(EquipLoadout { index, name, is_using, inventory_indices })
    })
    .collect()
}

/// Copies one 装备组合 register from the source to a slot in the target.
/// With `include_equipment`, every equipment box piece the loadout references
/// is copied source→target at the same box index, so the register keeps
/// pointing at the intended pieces. Returns the target slot used.
fn equip_my_set_list(payload: &SavePayload) -> Result<&Array> {
  let mut managers = Vec::new();
  collect_classes(payload, EQUIP_MANAGER_SAVE_CLASS, &mut managers);
  let manager = managers.first().ok_or_else(|| anyhow::anyhow!("no EquipDataManager in save"))?;
  match manager.fields.iter().find(|field| field.hash == EQUIP_MY_SET_LIST).map(|field| &field.value) {
    Some(FieldValue::Array(array)) => Ok(array),
    _ => bail!("EquipDataManager has no _PlEquipMySetList array"),
  }
}

/// Number of `_field_hash` arrays inside `class_hash` instances.
fn count_field_arrays(payload: &SavePayload, class_hash: u32, field_hash: u32) -> usize {
  fn count(class: &Class, class_hash: u32, field_hash: u32, out: &mut usize) {
    if class.hash == class_hash && class.fields.iter().any(|field| field.hash == field_hash) {
      *out += 1;
    }
    for field in &class.fields {
      match &field.value {
        FieldValue::Class(nested) => count(nested, class_hash, field_hash, out),
        FieldValue::Array(array) => {
          for element in &array.values {
            if let ArrayValue::Class(nested) = element {
              count(nested, class_hash, field_hash, out);
            }
          }
        }
        _ => {}
      }
    }
  }
  let mut out = 0;
  for entry in &payload.entries {
    count(&entry.class, class_hash, field_hash, &mut out);
  }
  out
}

/// Recursively replaces `element_index` of the array stored at
/// (`class_hash`, `field_hash`) with `replacement`, on every matching
/// instance (callers verify there is exactly one).
fn replace_list_element(
  payload: &mut SavePayload,
  class_hash: u32,
  field_hash: u32,
  element_index: usize,
  replacement: &ArrayValue,
) -> Result<()> {
  fn rec(
    class: &mut Class,
    class_hash: u32,
    field_hash: u32,
    element_index: usize,
    replacement: &ArrayValue,
    matched: &mut usize,
  ) -> Result<()> {
    let class_hash_here = class.hash;
    for field in &mut class.fields {
      if class_hash_here == class_hash && field.hash == field_hash {
        if let FieldValue::Array(array) = &mut field.value {
          if element_index >= array.values.len() {
            bail!("element index {element_index} out of range ({} slots)", array.values.len());
          }
          array.values[element_index] = replacement.clone();
          *matched += 1;
          continue;
        }
      }
      match &mut field.value {
        FieldValue::Class(nested) => {
          rec(nested, class_hash, field_hash, element_index, replacement, matched)?
        }
        FieldValue::Array(array) => {
          for element in &mut array.values {
            if let ArrayValue::Class(nested) = element {
              rec(nested, class_hash, field_hash, element_index, replacement, matched)?;
            }
          }
        }
        _ => {}
      }
    }
    Ok(())
  }
  let mut matched = 0;
  for entry in &mut payload.entries {
    rec(&mut entry.class, class_hash, field_hash, element_index, replacement, &mut matched)?;
  }
  if matched == 0 {
    bail!("no array at (class {class_hash:08x}, field {field_hash:08x})");
  }
  Ok(())
}

/// Recursively applies `copy` to the equipment box array of the target
/// payload. `source_slots` is the source box's slot count for the
/// length check.
fn with_box_array(
  target: &mut SavePayload,
  source_slots: usize,
  copy: &mut impl FnMut(&mut Array) -> Result<()>,
) -> Result<()> {
  fn rec(
    class: &mut Class,
    source_slots: usize,
    copy: &mut impl FnMut(&mut Array) -> Result<()>,
    matched: &mut usize,
  ) -> Result<()> {
    let class_hash_here = class.hash;
    for field in &mut class.fields {
      if class_hash_here == EQUIP_BOX_CLASS && field.hash == EQUIP_BOX_WEAPON_ARMOR_LIST {
        if let FieldValue::Array(array) = &mut field.value {
          if source_slots != array.values.len() {
            bail!("equipment box slot count differs between saves");
          }
          copy(array)?;
          *matched += 1;
          continue;
        }
      }
      match &mut field.value {
        FieldValue::Class(nested) => rec(nested, source_slots, copy, matched)?,
        FieldValue::Array(array) => {
          for element in &mut array.values {
            if let ArrayValue::Class(nested) = element {
              rec(nested, source_slots, copy, matched)?;
            }
          }
        }
        _ => {}
      }
    }
    Ok(())
  }
  let mut matched = 0;
  for entry in &mut target.entries {
    rec(&mut entry.class, source_slots, copy, &mut matched)?;
  }
  if matched == 0 {
    bail!("no EquipBox _WeaponArmorInventoryList array in the target save");
  }
  Ok(())
}

fn equip_armor_list(payload: &SavePayload) -> Result<&Array> {
  let mut boxes = Vec::new();
  collect_classes(payload, EQUIP_BOX_CLASS, &mut boxes);
  let box_class = boxes.first().ok_or_else(|| anyhow::anyhow!("no EquipBox in save"))?;
  match box_class
    .fields
    .iter()
    .find(|field| field.hash == EQUIP_BOX_WEAPON_ARMOR_LIST)
    .map(|field| &field.value)
  {
    Some(FieldValue::Array(array)) => Ok(array),
    _ => bail!("EquipBox has no _WeaponArmorInventoryList array"),
  }
}

fn inventory_indices_of(element: &ArrayValue) -> Vec<u32> {
  let ArrayValue::Class(class) = element else {
    return Vec::new();
  };
  let Some(FieldValue::Array(indices)) =
    class.fields.iter().find(|field| field.hash == EQUIP_MYSET_INVENTORY_INDEX_LIST).map(|field| &field.value)
  else {
    return Vec::new();
  };
  indices
    .values
    .iter()
    .filter_map(|element| match element {
      ArrayValue::Scalar(bytes) if bytes.len() == 4 => {
        Some(u32::from_le_bytes(bytes.as_slice().try_into().expect("4 bytes")))
      }
      _ => None,
    })
    .collect()
}

/// Copies one 装备组合 register from the source to a slot in the target.
/// With `include_equipment`, every equipment box piece the loadout references
/// is copied source→target at the same box index, so the register keeps
/// pointing at the intended pieces. Returns the target slot used.
pub fn copy_equip_loadout(
  target: &mut SavePayload,
  source: &SavePayload,
  source_index: usize,
  target_index: Option<usize>,
  include_equipment: bool,
) -> Result<usize> {
  if count_field_arrays(target, EQUIP_MANAGER_SAVE_CLASS, EQUIP_MY_SET_LIST) != 1 {
    bail!("target must contain exactly one _PlEquipMySetList");
  }
  let source_element = {
    let list = equip_my_set_list(source)?;
    if source_index >= list.values.len() {
      bail!("source loadout index {source_index} out of range ({} slots)", list.values.len());
    }
    list.values[source_index].clone()
  };
  let referenced_indices = inventory_indices_of(&source_element);

  let target_slot = match target_index {
    Some(slot) => slot,
    None => {
      let loadouts = read_equip_loadouts(target);
      loadouts
        .iter()
        .find(|loadout| !loadout.is_using)
        .map(|loadout| loadout.index)
        .ok_or_else(|| anyhow::anyhow!("target has no free loadout slot; specify a slot to overwrite"))?
    }
  };

  if include_equipment {
    let source_box = equip_armor_list(source)?;
    let source_slots = source_box.values.len();
    let indices = referenced_indices.clone();
    with_box_array(target, source_slots, &mut |target_box| {
      for &box_index in &indices {
        let index = box_index as usize;
        if index < target_box.values.len() {
          target_box.values[index] = source_box.values[index].clone();
        }
      }
      Ok(())
    })?;
  }

  replace_list_element(target, EQUIP_MANAGER_SAVE_CLASS, EQUIP_MY_SET_LIST, target_slot, &source_element)?;
  Ok(target_slot)
}

/// Copies a single equipment piece between box slots: source box
/// `source_index` is written to target box `target_index`.
pub fn copy_equip_piece(
  target: &mut SavePayload,
  source: &SavePayload,
  source_index: usize,
  target_index: usize,
) -> Result<()> {
  let source_value = {
    let source_box = equip_armor_list(source)?;
    source_box.values.get(source_index).cloned().ok_or_else(|| {
      anyhow::anyhow!("source box index {source_index} out of range ({} slots)", source_box.values.len())
    })?
  };
  let source_slots = equip_armor_list(source)?.values.len();
  with_box_array(target, source_slots, &mut |target_box| {
    if target_index >= target_box.values.len() {
      bail!("target box index {target_index} out of range ({} slots)", target_box.values.len());
    }
    target_box.values[target_index] = source_value.clone();
    Ok(())
  })?;
  Ok(())
}

/// The three progression ranks, from `ProgressSaveData`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ranks {
  pub hunter: u32,
  pub master: u32,
  pub mystery_research: u32,
}

/// Reads the three ranks from `ProgressSaveData` (the authoritative copy).
pub fn read_ranks(payload: &SavePayload) -> Option<Ranks> {
  let mut instances = Vec::new();
  collect_classes(payload, PROGRESS_SAVE_CLASS, &mut instances);
  if instances.len() != 1 {
    return None;
  }
  let read = |field_hash: u32| -> Option<u32> {
    instances[0]
      .fields
      .iter()
      .find(|field| field.hash == field_hash)
      .and_then(|field| match &field.value {
        FieldValue::Scalar { bytes, size: 4 } if bytes.len() == 4 => {
          Some(u32::from_le_bytes(bytes.as_slice().try_into().expect("4 bytes")))
        }
        _ => None,
      })
  };
  Some(Ranks {
    hunter: read(HUNTER_RANK)?,
    master: read(MASTER_RANK)?,
    mystery_research: read(MYSTERY_RESEARCH_LEVEL)?,
  })
}

/// Sets the three ranks. Writes both the authoritative `ProgressSaveData`
/// values and the self guild card's display copies so the title screen and
/// the progression system agree. Buddy guild cards (nested in arrays) are
/// never touched, and the point accumulators are left alone.
pub fn set_ranks(
  payload: &mut SavePayload,
  hunter: u32,
  master: u32,
  mystery_research: u32,
) -> Result<Ranks> {
  let before = read_ranks(payload)
    .ok_or_else(|| anyhow::anyhow!("ProgressSaveData not found or ambiguous; refusing to write ranks"))?;
  set_scalar(payload, PROGRESS_SAVE_CLASS, HUNTER_RANK, hunter, "_HunterRank")?;
  set_scalar(payload, PROGRESS_SAVE_CLASS, MASTER_RANK, master, "_MasterRank")?;
  set_scalar(payload, PROGRESS_SAVE_CLASS, MYSTERY_RESEARCH_LEVEL, mystery_research, "_MysteryResearchLevel")?;

  let updates = [
    (HUNTER_RANK, hunter),
    (MASTER_RANK, master),
    (GUILD_CARD_LABO_LV, mystery_research),
  ];
  fn update(class: &mut Class, updates: &[(u32, u32)], updated: &mut usize) {
    if class.hash == GUILD_CARD_CLASS {
      for field in &mut class.fields {
        if let Some(&(_, value)) = updates.iter().find(|(hash, _)| *hash == field.hash)
          && let FieldValue::Scalar { size: 4, bytes } = &mut field.value
          && bytes.len() == 4
        {
          *bytes = value.to_le_bytes().to_vec();
          *updated += 1;
        }
      }
    }
    // Class fields only: buddy guild cards live inside arrays.
    for field in &mut class.fields {
      if let FieldValue::Class(nested) = &mut field.value {
        update(nested, updates, updated);
      }
    }
  }
  let mut updated = 0usize;
  for entry in &mut payload.entries {
    update(&mut entry.class, &updates, &mut updated);
  }
  if updated == 0 {
    bail!("self guild card not found; refusing to write ranks");
  }
  Ok(before)
}

#[derive(Debug, Clone, PartialEq)]
pub struct EquipmentTransferReport {
  /// (class label, number of fields copied) per transferred class.
  pub classes: Vec<(&'static str, usize)>,
  pub box_slots: usize,
  pub box_used: usize,
}

/// Which equipment classes a partial transfer copies. Selecting a subset can
/// leave the target's index references (`PlEquipPack` box indices, loadout
/// registers) pointing into a box that no longer holds the same pieces — the
/// game tolerates this, but registered sets then resolve to different gear.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EquipParts {
  /// `snow.data.EquipBox` — stored weapons, armor, talismans.
  pub equip_box: bool,
  /// `snow.data.EquipDataManager.SaveData1` — worn pack, loadout registers,
  /// hunter sets.
  pub equip_manager: bool,
  /// `snow.data.ItemMySet` — the 40 item pouch loadouts.
  pub item_my_set: bool,
}

impl Default for EquipParts {
  fn default() -> Self {
    Self { equip_box: true, equip_manager: true, item_my_set: true }
  }
}

impl EquipParts {
  pub fn all() -> Self {
    Self::default()
  }

  pub fn none() -> Self {
    Self { equip_box: false, equip_manager: false, item_my_set: false }
  }

  fn any(self) -> bool {
    self.equip_box || self.equip_manager || self.item_my_set
  }
}

/// Per-slot item box merge result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MergeReport {
  /// Total slots in the box (target and source must agree).
  pub slots: usize,
  /// Slots taken from the source (item passed the filter).
  pub transferred: usize,
  /// Slots left as the target had them.
  pub kept: usize,
  /// Non-empty slots in the source box.
  pub source_items: usize,
}

/// Transfers the whole equipment complex from source to target:
///
/// - `snow.data.EquipBox` — stored weapons, armor, talismans (with skills,
///   decorations, augmentation data) plus the Qurious Crafting cage;
/// - `snow.data.EquipDataManager.SaveData1` — the worn equipment pack (box
///   index references), the 224 equipment loadouts (`PlEquipMySetData`), the
///   224 hunter sets (`HunterMySetData`) and overwear/color sets;
/// - `snow.data.ItemMySet` — the 40 item pouch loadouts referenced by the
///   hunter sets.
///
/// Copying these three classes together keeps every index reference
/// (`PlEquipPack`, `_InventoryIndexList`, `_ItemMysetIndex`,
/// `_EquipMysetIndex`) internally consistent.
pub fn transfer_equipment(
  target: &mut SavePayload,
  source: &SavePayload,
) -> Result<EquipmentTransferReport> {
  transfer_equipment_parts(target, source, EquipParts::all())
}

/// Transfers only the selected equipment classes. See `transfer_equipment`
/// for the full-complex copy; partial copies can leave index references
/// pointing at different box contents.
pub fn transfer_equipment_parts(
  target: &mut SavePayload,
  source: &SavePayload,
  parts: EquipParts,
) -> Result<EquipmentTransferReport> {
  if !parts.any() {
    bail!("no equipment components selected; nothing to transfer");
  }
  let mut classes = Vec::new();
  if parts.equip_box {
    classes.push(("EquipBox", copy_class_fields(target, source, EQUIP_BOX_CLASS)?));
  }
  if parts.equip_manager {
    classes.push(("EquipManager", copy_class_fields(target, source, EQUIP_MANAGER_SAVE_CLASS)?));
  }
  if parts.item_my_set {
    classes.push(("ItemMySet", copy_class_fields(target, source, ITEM_MY_SET_CLASS)?));
  }

  let mut boxes = Vec::new();
  collect_classes(target, EQUIP_BOX_CLASS, &mut boxes);
  let mut box_slots = 0;
  let mut box_used = 0;
  if let Some(box_class) = boxes.first() {
    for field in &box_class.fields {
      if field.hash == EQUIP_BOX_WEAPON_ARMOR_LIST
        && let FieldValue::Array(array) = &field.value
      {
        box_slots = array.values.len();
        box_used = count_used_equipment(array);
      }
    }
  }
  Ok(EquipmentTransferReport { classes, box_slots, box_used })
}

/// Replaces the fields of the target's single `class_hash` instance with the
/// source's. Both payloads must contain exactly one instance of the class.
pub fn copy_class_fields(
  target: &mut SavePayload,
  source: &SavePayload,
  class_hash: u32,
) -> Result<usize> {
  let mut source_classes = Vec::new();
  collect_classes(source, class_hash, &mut source_classes);
  if source_classes.len() != 1 {
    bail!(
      "source contains {} instances of class {class_hash:08x}; expected exactly one",
      source_classes.len()
    );
  }
  let fields = source_classes[0].fields.clone();

  let mut replaced = 0usize;
  for entry in &mut target.entries {
    assign_in_class_mut(&mut entry.class, class_hash, &fields, &mut replaced);
  }
  if replaced != 1 {
    bail!("target contains {replaced} instances of class {class_hash:08x}; expected exactly one",);
  }
  Ok(fields.len())
}

fn assign_in_class_mut(class: &mut Class, class_hash: u32, fields: &[Field], replaced: &mut usize) {
  if class.hash == class_hash {
    class.fields = fields.to_vec();
    *replaced += 1;
  }
  for field in &mut class.fields {
    match &mut field.value {
      FieldValue::Class(nested) => assign_in_class_mut(nested, class_hash, fields, replaced),
      FieldValue::Array(array) => {
        for element in &mut array.values {
          if let ArrayValue::Class(nested) = element {
            assign_in_class_mut(nested, class_hash, fields, replaced);
          }
        }
      }
      _ => {}
    }
  }
}

fn collect_classes<'a>(payload: &'a SavePayload, class_hash: u32, out: &mut Vec<&'a Class>) {
  for entry in &payload.entries {
    collect_in_class(&entry.class, class_hash, out);
  }
}

/// Diagnostic access for examples: every instance of `class_hash`.
pub fn collect_classes_public<'a>(
  payload: &'a SavePayload,
  class_hash: u32,
  out: &mut Vec<&'a Class>,
) {
  collect_classes(payload, class_hash, out)
}

fn collect_in_class<'a>(class: &'a Class, class_hash: u32, out: &mut Vec<&'a Class>) {
  if class.hash == class_hash {
    out.push(class);
  }
  for field in &class.fields {
    match &field.value {
      FieldValue::Class(nested) => collect_in_class(nested, class_hash, out),
      FieldValue::Array(array) => {
        for element in &array.values {
          if let ArrayValue::Class(nested) = element {
            collect_in_class(nested, class_hash, out);
          }
        }
      }
      _ => {}
    }
  }
}

fn count_used_equipment(array: &Array) -> usize {
  array
    .values
    .iter()
    .filter(|element| {
      if let ArrayValue::Class(class) = element {
        return class.fields.iter().any(|field| {
          field.hash == EQUIP_ID_VAL
            && matches!(&field.value, FieldValue::Scalar { bytes, .. } if bytes.iter().any(|&byte| byte != 0))
        });
      }
      false
    })
    .count()
}

#[derive(Debug, Clone, PartialEq)]
pub struct EditReport {
  pub field: &'static str,
  pub updated: usize,
  pub before: Vec<u32>,
  pub after: u32,
}

/// Sets the current wallet amount; optionally the lifetime counter as well.
pub fn set_money(
  payload: &mut SavePayload,
  value: u32,
  total_added: Option<u32>,
) -> Result<EditReport> {
  let mut report = set_scalar(payload, HAND_MONEY_CLASS, HAND_MONEY_VALUE, value, "_Value")?;
  if let Some(total) = total_added {
    let total_report =
      set_scalar(payload, HAND_MONEY_CLASS, HAND_MONEY_TOTAL_ADDED, total, "_TotalAddedValue")?;
    report.updated += total_report.updated;
    report.before.extend(total_report.before);
  }
  Ok(report)
}

/// `snow.data.MyData.HunterName` — the hunter's display name.
pub const HUNTER_NAME_FIELD: u32 = 0x02bb_0110;

/// Reads a u32 scalar from the first class instance matching `class_hash` +
/// `field_hash`.
pub fn read_scalar(payload: &SavePayload, class_hash: u32, field_hash: u32) -> Option<u32> {
  let mut classes = Vec::new();
  collect_classes(payload, class_hash, &mut classes);
  let class = classes.first()?;
  class.fields.iter().find(|field| field.hash == field_hash).and_then(|field| match &field.value {
    FieldValue::Scalar { size: 4, bytes } if bytes.len() == 4 => {
      Some(u32::from_le_bytes(bytes.as_slice().try_into().expect("4 bytes")))
    }
    _ => None,
  })
}

/// Reads the current wallet amount and the lifetime money-gained counter.
pub fn read_money(payload: &SavePayload) -> Option<(u32, u32)> {
  Some((
    read_scalar(payload, HAND_MONEY_CLASS, HAND_MONEY_VALUE)?,
    read_scalar(payload, HAND_MONEY_CLASS, HAND_MONEY_TOTAL_ADDED)?,
  ))
}

/// Reads the current Kamura/Steady point balance.
pub fn read_points(payload: &SavePayload) -> Option<u32> {
  read_scalar(payload, VILLAGE_POINT_CLASS, VILLAGE_POINT_VALUE)
}

/// Reads the hunter's display name (`snow.data.ProfileData.HunterName`).
/// Buddy lists reuse the same field for buddy names, so the first match in
/// document order — the hunter's own profile — is returned.
pub fn read_hunter_name(payload: &SavePayload) -> Option<String> {
  fn walk(class: &Class, out: &mut Option<String>) {
    if out.is_some() {
      return;
    }
    for field in &class.fields {
      if field.hash == HUNTER_NAME_FIELD
        && let FieldValue::String(units) = &field.value
      {
        *out = Some(String::from_utf16_lossy(units));
        return;
      }
      match &field.value {
        FieldValue::Class(nested) => walk(nested, out),
        FieldValue::Array(array) => {
          for element in &array.values {
            if let ArrayValue::Class(nested) = element {
              walk(nested, out);
              if out.is_some() {
                return;
              }
            }
          }
        }
        _ => {}
      }
      if out.is_some() {
        return;
      }
    }
  }
  let mut out = None;
  for entry in &payload.entries {
    walk(&entry.class, &mut out);
    if out.is_some() {
      break;
    }
  }
  out
}

/// Sets the current Kamura/Steady point balance.
pub fn set_village_points(payload: &mut SavePayload, value: u32) -> Result<EditReport> {
  set_scalar(payload, VILLAGE_POINT_CLASS, VILLAGE_POINT_VALUE, value, "_Point")
}

#[derive(Debug, Clone, PartialEq)]
pub struct TransferReport {
  pub slots: usize,
  pub items: usize,
}

/// Replaces the target's item box with the source's item box, slot for slot.
///
/// The box is a fixed-capacity array of `(ItemId, Num)` slots; empty slots
/// carry `Num == 0` and are copied as-is, so the target ends up with exactly
/// the source's contents.
pub fn transfer_item_box(target: &mut SavePayload, source: &SavePayload) -> Result<TransferReport> {
  let source_array = find_inventory_list(source)?;
  let items = count_nonempty_slots(source_array);
  let slots = source_array.values.len();

  let mut replaced = 0usize;
  for entry in &mut target.entries {
    replace_in_class(&mut entry.class, source_array, &mut replaced)?;
  }
  match replaced {
    1 => Ok(TransferReport { slots, items }),
    0 => bail!("no item box (_InventoryList) found in the target save"),
    many => bail!("found {many} item box inventories in the target save; expected exactly one"),
  }
}

/// The item id stored in one item box slot (`ItemInventoryData._ItemCount._Id`).
fn slot_item_id(slot: &ArrayValue) -> Option<u32> {
  let ArrayValue::Class(class) = slot else {
    return None;
  };
  for field in &class.fields {
    if field.hash != ITEM_INVENTORY_COUNT {
      continue;
    }
    let FieldValue::Class(count) = &field.value else {
      return None;
    };
    for number_field in &count.fields {
      if number_field.hash != ITEM_COUNT_ID {
        continue;
      }
      if let FieldValue::Scalar { bytes, .. } = &number_field.value {
        return Some(u32::from_le_bytes(bytes.as_slice().try_into().ok()?));
      }
    }
  }
  None
}

/// Merge variant of [`transfer_item_box`]: only slots whose source item
/// passes `keep` are copied; everything else keeps the target's contents.
/// Both boxes must have the same slot count (the game allocates a fixed
/// 1800), matching the layout check `transfer_item_box` performs.
pub fn transfer_item_box_filtered(
  target: &mut SavePayload,
  source: &SavePayload,
  keep: impl Fn(u32) -> bool,
) -> Result<MergeReport> {
  let source_array = find_inventory_list(source)?;

  fn merge_in_class(
    class: &mut Class,
    source_array: &Array,
    keep: &impl Fn(u32) -> bool,
    transferred: &mut usize,
    merged_boxes: &mut usize,
    slots: &mut usize,
  ) -> Result<()> {
    let class_hash = class.hash;
    for field in &mut class.fields {
      if class_hash == ITEM_BOX_CLASS && field.hash == ITEM_BOX_INVENTORY_LIST {
        if let FieldValue::Array(target_array) = &mut field.value {
          if source_array.member_type != target_array.member_type
            || source_array.member_size != target_array.member_size
            || source_array.array_type != target_array.array_type
          {
            bail!("source and target item boxes have different array layouts; refusing to transfer");
          }
          if source_array.values.len() != target_array.values.len() {
            bail!(
              "source box has {} slots but target has {}; game versions may differ",
              source_array.values.len(),
              target_array.values.len()
            );
          }
          *slots = target_array.values.len();
          for (target_slot, source_slot) in
            target_array.values.iter_mut().zip(&source_array.values)
          {
            if slot_item_id(source_slot).is_some_and(|id| keep(id)) {
              *target_slot = source_slot.clone();
              *transferred += 1;
            }
          }
          *merged_boxes += 1;
          continue;
        }
      }
      match &mut field.value {
        FieldValue::Class(nested) => {
          merge_in_class(nested, source_array, keep, transferred, merged_boxes, slots)?
        }
        FieldValue::Array(array) => {
          for element in &mut array.values {
            if let ArrayValue::Class(nested) = element {
              merge_in_class(nested, source_array, keep, transferred, merged_boxes, slots)?;
            }
          }
        }
        _ => {}
      }
    }
    Ok(())
  }

  let mut transferred = 0usize;
  let mut merged_boxes = 0usize;
  let mut slots = 0usize;
  for entry in &mut target.entries {
    merge_in_class(
      &mut entry.class,
      source_array,
      &keep,
      &mut transferred,
      &mut merged_boxes,
      &mut slots,
    )?;
  }
  match merged_boxes {
    1 => {
      let source_items = count_nonempty_slots(source_array);
      Ok(MergeReport { slots, transferred, kept: slots - transferred, source_items })
    }
    0 => bail!("no item box (_InventoryList) found in the target save"),
    many => bail!("found {many} item box inventories in the target save; expected exactly one"),
  }
}

fn find_inventory_list(payload: &SavePayload) -> Result<&Array> {
  let mut matches = Vec::new();
  for entry in &payload.entries {
    find_in_class(&entry.class, &mut matches);
  }
  match matches.as_slice() {
    [only] => Ok(only),
    [] => bail!("no item box (_InventoryList) found in the source save"),
    many => {
      bail!("found {} item box inventories in the source save; expected exactly one", many.len())
    }
  }
}

fn find_in_class<'a>(class: &'a Class, matches: &mut Vec<&'a Array>) {
  let class_hash = class.hash;
  for field in &class.fields {
    if class_hash == ITEM_BOX_CLASS
      && field.hash == ITEM_BOX_INVENTORY_LIST
      && let FieldValue::Array(array) = &field.value
    {
      matches.push(array);
    }
    match &field.value {
      FieldValue::Class(nested) => find_in_class(nested, matches),
      FieldValue::Array(array) => {
        for element in &array.values {
          if let ArrayValue::Class(nested) = element {
            find_in_class(nested, matches);
          }
        }
      }
      _ => {}
    }
  }
}

fn replace_in_class(class: &mut Class, source_array: &Array, replaced: &mut usize) -> Result<()> {
  let class_hash = class.hash;
  for field in &mut class.fields {
    if class_hash == ITEM_BOX_CLASS
      && field.hash == ITEM_BOX_INVENTORY_LIST
      && let FieldValue::Array(target_array) = &mut field.value
    {
      if source_array.member_type != target_array.member_type
        || source_array.member_size != target_array.member_size
        || source_array.array_type != target_array.array_type
      {
        bail!("source and target item boxes have different array layouts; refusing to transfer");
      }
      if source_array.values.len() != target_array.values.len() {
        bail!(
          "source box has {} slots but target has {}; game versions may differ",
          source_array.values.len(),
          target_array.values.len()
        );
      }
      target_array.values = source_array.values.clone();
      *replaced += 1;
    }
    match &mut field.value {
      FieldValue::Class(nested) => replace_in_class(nested, source_array, replaced)?,
      FieldValue::Array(array) => {
        for element in &mut array.values {
          if let ArrayValue::Class(nested) = element {
            replace_in_class(nested, source_array, replaced)?;
          }
        }
      }
      _ => {}
    }
  }
  Ok(())
}

fn count_nonempty_slots(array: &Array) -> usize {
  array
    .values
    .iter()
    .filter(|element| {
      let ArrayValue::Class(class) = element else {
        return false;
      };
      class.fields.iter().any(|field| {
        if field.hash != ITEM_INVENTORY_COUNT {
          return false;
        }
        let FieldValue::Class(count) = &field.value else {
          return false;
        };
        count.fields.iter().any(|number_field| {
          if number_field.hash != ITEM_COUNT_NUM {
            return false;
          }
          let FieldValue::Scalar { bytes, .. } = &number_field.value else {
            return false;
          };
          bytes.iter().any(|byte| *byte != 0)
        })
      })
    })
    .count()
}

#[derive(Debug, Clone, PartialEq)]
pub struct ItemBoxEntry {
  pub id: u32,
  pub num: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ItemEditReport {
  /// Number of requested changes that found a slot.
  pub applied: usize,
  /// Number of previously empty slots filled with a new item.
  pub filled: usize,
}

/// Reads the item box as `(item id, count)` pairs in slot order. Empty slots
/// have `num == 0`.
pub fn read_item_box(payload: &SavePayload) -> Result<Vec<ItemBoxEntry>> {
  let array = find_inventory_list(payload)?;
  let mut entries = Vec::with_capacity(array.values.len());
  for element in &array.values {
    let ArrayValue::Class(slot) = element else {
      continue;
    };
    let mut id = 0;
    let mut num = 0;
    for slot_field in &slot.fields {
      if slot_field.hash != ITEM_INVENTORY_COUNT {
        continue;
      }
      let FieldValue::Class(count) = &slot_field.value else {
        continue;
      };
      for field in &count.fields {
        let FieldValue::Scalar { size, bytes } = &field.value else {
          continue;
        };
        if *size != 4 || bytes.len() != 4 {
          continue;
        }
        let value = u32::from_le_bytes(bytes.as_slice().try_into().expect("4 bytes"));
        if field.hash == ITEM_COUNT_ID {
          id = value;
        } else if field.hash == ITEM_COUNT_NUM {
          num = value;
        }
      }
    }
    entries.push(ItemBoxEntry { id, num });
  }
  Ok(entries)
}

/// Applies per-item count changes to the item box: a slot carrying the item's
/// id is updated in place; items not yet in the box fill empty (`num == 0`)
/// slots. Setting a count to 0 empties the item's slot.
pub fn set_item_counts(
  payload: &mut SavePayload,
  changes: &[(u32, u32)],
) -> Result<ItemEditReport> {
  let mut pending: std::collections::BTreeMap<u32, u32> = changes.iter().copied().collect();
  let mut report = ItemEditReport { applied: 0, filled: 0 };

  // Pass 1: update slots whose id matches a pending change. Clearing (0)
  // also restores the "none" sentinel so the slot matches the game's own
  // empty-slot representation.
  for_each_item_count_mut(payload, &mut |count| {
    let id = read_count_field(count, ITEM_COUNT_ID);
    if let Some(number) = pending.get(&id).copied() {
      if number == 0 {
        write_count_field(count, ITEM_COUNT_ID, ITEM_ID_NONE);
      }
      write_count_field(count, ITEM_COUNT_NUM, number);
      pending.remove(&id);
      report.applied += 1;
    }
  });

  // Pass 2: place remaining new items into empty slots (`_Num == 0`).
  if !pending.is_empty() {
    let mut fill_order: Vec<u32> =
      pending.iter().filter(|(_, num)| **num > 0).map(|(id, _)| *id).collect();
    fill_order.sort_unstable();
    let mut fill_iter = fill_order.into_iter();
    let mut next_fill = fill_iter.next();
    for_each_item_count_mut(payload, &mut |count| {
      let Some(id) = next_fill else { return };
      let Some(number) = pending.get(&id).copied() else { return };
      if read_count_field(count, ITEM_COUNT_NUM) == 0 {
        write_count_field(count, ITEM_COUNT_ID, id);
        write_count_field(count, ITEM_COUNT_NUM, number);
        pending.remove(&id);
        next_fill = fill_iter.next();
        report.applied += 1;
        report.filled += 1;
      }
    });
  }

  let unplaced: Vec<u32> = pending.iter().filter(|(_, num)| **num > 0).map(|(id, _)| *id).collect();
  if !unplaced.is_empty() {
    bail!(
      "item box is full: {} new item(s) could not be placed (ids: {:08x?})",
      unplaced.len(),
      unplaced
    );
  }
  Ok(report)
}

/// Walks every `snow.data.ItemCount` class inside the item box.
fn for_each_item_count_mut(payload: &mut SavePayload, f: &mut dyn FnMut(&mut Class)) {
  for entry in &mut payload.entries {
    for_each_item_count_in_class(&mut entry.class, f);
  }
}

fn for_each_item_count_in_class(class: &mut Class, f: &mut dyn FnMut(&mut Class)) {
  let class_hash = class.hash;
  for field in &mut class.fields {
    if class_hash == ITEM_BOX_CLASS
      && field.hash == ITEM_BOX_INVENTORY_LIST
      && let FieldValue::Array(array) = &mut field.value
    {
      for element in &mut array.values {
        if let ArrayValue::Class(slot) = element {
          for slot_field in &mut slot.fields {
            if slot_field.hash == ITEM_INVENTORY_COUNT
              && let FieldValue::Class(count) = &mut slot_field.value
            {
              f(count);
            }
          }
        }
      }
    }
    match &mut field.value {
      FieldValue::Class(nested) => for_each_item_count_in_class(nested, f),
      FieldValue::Array(array) => {
        for element in &mut array.values {
          if let ArrayValue::Class(nested) = element {
            for_each_item_count_in_class(nested, f);
          }
        }
      }
      _ => {}
    }
  }
}

fn read_count_field(count: &Class, field_hash: u32) -> u32 {
  for field in &count.fields {
    if field.hash == field_hash
      && let FieldValue::Scalar { size: 4, bytes } = &field.value
      && bytes.len() == 4
    {
      return u32::from_le_bytes(bytes.as_slice().try_into().expect("4 bytes"));
    }
  }
  0
}

fn write_count_field(count: &mut Class, field_hash: u32, value: u32) {
  for field in &mut count.fields {
    if field.hash == field_hash
      && let FieldValue::Scalar { size: 4, bytes } = &mut field.value
      && bytes.len() == 4
    {
      *bytes = value.to_le_bytes().to_vec();
      return;
    }
  }
}

/// Rewrites every u32 scalar at `class_hash` + `field_hash` to `value`,
/// returning the previous values. Refuses to write nothing: a zero match
/// means the assumed schema drifted and an edit would be a silent no-op.
pub fn set_scalar(
  payload: &mut SavePayload,
  class_hash: u32,
  field_hash: u32,
  value: u32,
  label: &'static str,
) -> Result<EditReport> {
  let mut before = Vec::new();
  let mut updated = 0usize;
  for entry in &mut payload.entries {
    update_class(&mut entry.class, class_hash, field_hash, value, &mut before, &mut updated);
  }
  if updated == 0 {
    bail!(
      "no field {label} (class {class_hash:08x}, field {field_hash:08x}) found; refusing to write"
    );
  }
  Ok(EditReport { field: label, updated, before, after: value })
}

fn update_class(
  class: &mut Class,
  class_hash: u32,
  field_hash: u32,
  value: u32,
  before: &mut Vec<u32>,
  updated: &mut usize,
) {
  if class.hash == class_hash {
    for field in &mut class.fields {
      if field.hash == field_hash
        && let FieldValue::Scalar { size, bytes } = &mut field.value
        && *size == 4
        && bytes.len() == 4
      {
        before.push(u32::from_le_bytes(bytes.as_slice().try_into().expect("4 bytes")));
        *bytes = value.to_le_bytes().to_vec();
        *updated += 1;
      }
    }
  }
  for field in &mut class.fields {
    match &mut field.value {
      FieldValue::Class(nested) => {
        update_class(nested, class_hash, field_hash, value, before, updated);
      }
      FieldValue::Array(array) => {
        for element in &mut array.values {
          if let crate::payload::ArrayValue::Class(nested) = element {
            update_class(nested, class_hash, field_hash, value, before, updated);
          }
        }
      }
      _ => {}
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::payload::{Field, NativeClass};

  fn money_payload(value: u32, total: u32) -> SavePayload {
    let money_class = Class {
      hash: HAND_MONEY_CLASS,
      fields: vec![
        Field { hash: HAND_MONEY_VALUE, field_type: 0x08, value: scalar(value) },
        Field { hash: HAND_MONEY_TOTAL_ADDED, field_type: 0x09, value: scalar(total) },
      ],
    };
    let other_class = Class {
      hash: 0xdead_beef,
      fields: vec![Field { hash: HAND_MONEY_VALUE, field_type: 0x08, value: scalar(111) }],
    };
    SavePayload {
      entries: vec![
        NativeClass { native_hash: 0x1, class: other_class },
        NativeClass {
          native_hash: 0x2,
          class: Class {
            hash: 0x3,
            fields: vec![Field {
              hash: 0x4,
              field_type: 0x11,
              value: FieldValue::Class(Box::new(money_class)),
            }],
          },
        },
      ],
    }
  }

  fn scalar(value: u32) -> FieldValue {
    FieldValue::Scalar { size: 4, bytes: value.to_le_bytes().to_vec() }
  }

  #[test]
  fn sets_money_only_inside_matching_class() {
    let mut payload = money_payload(7_368_280, 15_979_450);
    let report = set_money(&mut payload, 99_999_999, None).expect("money should be set");
    assert_eq!(report.updated, 1);
    assert_eq!(report.before, vec![7_368_280]);

    // The same-named field in the unrelated class must stay untouched.
    let other = &payload.entries[0].class.fields[0].value;
    assert_eq!(*other, scalar(111));
    let money_class = match &payload.entries[1].class.fields[0].value {
      FieldValue::Class(class) => class,
      other => panic!("expected nested class, got {other:?}"),
    };
    let FieldValue::Scalar { bytes, .. } = &money_class.fields[0].value else {
      panic!("expected scalar");
    };
    assert_eq!(bytes.as_slice(), 99_999_999u32.to_le_bytes());
    // _TotalAddedValue untouched when not requested.
    let FieldValue::Scalar { bytes: total, .. } = &money_class.fields[1].value else {
      panic!("expected scalar");
    };
    assert_eq!(total.as_slice(), 15_979_450u32.to_le_bytes());
  }

  #[test]
  fn set_money_can_also_update_lifetime_counter() {
    let mut payload = money_payload(1, 2);
    let report = set_money(&mut payload, 3, Some(4)).expect("money should be set");
    assert_eq!(report.updated, 2);
    assert_eq!(report.before, vec![1, 2]);
  }

  #[test]
  fn refuses_when_schema_drifted() {
    let mut payload = SavePayload { entries: vec![] };
    let error = set_money(&mut payload, 5, None).expect_err("empty payload must be refused");
    assert!(error.to_string().contains("refusing to write"), "unexpected error: {error}");
  }

  fn item_slot(id: u32, num: u32) -> crate::payload::ArrayValue {
    use crate::payload::ArrayValue;
    let count_class = Class {
      hash: 0x54d2_e337, // snow.data.ItemCount
      fields: vec![
        Field { hash: 0xaf48_5d0f, field_type: 0x08, value: scalar(id) }, // _Id
        Field { hash: ITEM_COUNT_NUM, field_type: 0x07, value: scalar(num) }, // _Num
      ],
    };
    let inventory_class = Class {
      hash: 0x24d2_65f2, // snow.data.ItemInventoryData
      fields: vec![Field {
        hash: ITEM_INVENTORY_COUNT,
        field_type: 0x11,
        value: FieldValue::Class(Box::new(count_class)),
      }],
    };
    ArrayValue::Class(Box::new(inventory_class))
  }

  fn box_payload(slots: Vec<crate::payload::ArrayValue>) -> SavePayload {
    use crate::payload::{Array, NativeClass};
    let inventory_array = Array {
      member_type: 0x11,
      member_size: 0,
      array_type: 1,
      class_hashes: Some(vec![0x24d2_65f2; slots.len()]),
      values: slots,
    };
    let item_box = Class {
      hash: ITEM_BOX_CLASS,
      fields: vec![Field {
        hash: ITEM_BOX_INVENTORY_LIST,
        field_type: -1,
        value: FieldValue::Array(inventory_array),
      }],
    };
    SavePayload {
      entries: vec![NativeClass {
        native_hash: 0x1,
        class: Class {
          hash: 0x3,
          fields: vec![Field {
            hash: 0x4,
            field_type: 0x11,
            value: FieldValue::Class(Box::new(item_box)),
          }],
        },
      }],
    }
  }

  #[test]
  fn transfers_item_box_slot_for_slot() {
    let source = box_payload(vec![item_slot(0x0410_0006, 1071), item_slot(0x0410_0007, 835)]);
    let mut target = box_payload(vec![item_slot(0x0410_0006, 0), item_slot(0x0410_0007, 0)]);
    let report = transfer_item_box(&mut target, &source).expect("transfer should succeed");
    assert_eq!(report.slots, 2);
    assert_eq!(report.items, 2);
    assert_eq!(
      find_inventory_list(&target).expect("target still has a box"),
      find_inventory_list(&source).expect("source has a box")
    );
  }

  #[test]
  fn transfer_requires_exactly_one_item_box() {
    let source = box_payload(vec![item_slot(1, 1)]);
    let mut empty = SavePayload { entries: vec![] };
    let error = transfer_item_box(&mut empty, &source).expect_err("target without a box must fail");
    assert!(error.to_string().contains("no item box"), "unexpected error: {error}");
    let _ = item_slot(0, 0);
  }

  #[test]
  fn merge_transfer_copies_only_slots_passing_the_filter() {
    // Source: 回复药 x10, 回复药G x5; target keeps 回复药G, takes 回复药.
    let source = box_payload(vec![item_slot(0x0410_0006, 10), item_slot(0x0410_0007, 5)]);
    let mut target = box_payload(vec![item_slot(0x0410_0006, 1), item_slot(0x0410_0007, 2)]);
    let report = transfer_item_box_filtered(&mut target, &source, |id| id == 0x0410_0006)
      .expect("merge should succeed");
    assert_eq!(report.slots, 2);
    assert_eq!(report.transferred, 1);
    assert_eq!(report.kept, 1);
    assert_eq!(report.source_items, 2);

    let entries = read_item_box(&target).expect("read merged box");
    assert_eq!(entries[0], ItemBoxEntry { id: 0x0410_0006, num: 10 });
    assert_eq!(entries[1], ItemBoxEntry { id: 0x0410_0007, num: 2 });
  }

  #[test]
  fn merge_transfer_rejects_mismatched_slot_counts() {
    let source = box_payload(vec![item_slot(1, 1), item_slot(2, 2)]);
    let mut target = box_payload(vec![item_slot(1, 1)]);
    let error = transfer_item_box_filtered(&mut target, &source, |_| true)
      .expect_err("slot count mismatch must fail");
    assert!(
      error.to_string().contains("2 slots but target has 1"),
      "unexpected error: {error}"
    );
  }

  fn ranks_payload(hunter: u32, master: u32, mystery: u32) -> SavePayload {
    let progress = Class {
      hash: PROGRESS_SAVE_CLASS,
      fields: vec![
        Field { hash: HUNTER_RANK, field_type: 0x08, value: scalar(hunter) },
        Field { hash: MASTER_RANK, field_type: 0x08, value: scalar(master) },
        Field { hash: MYSTERY_RESEARCH_LEVEL, field_type: 0x08, value: scalar(mystery) },
      ],
    };
    let guild_card = Class {
      hash: GUILD_CARD_CLASS,
      fields: vec![
        Field { hash: HUNTER_RANK, field_type: 0x08, value: scalar(hunter) },
        Field { hash: MASTER_RANK, field_type: 0x08, value: scalar(master) },
        Field { hash: GUILD_CARD_LABO_LV, field_type: 0x08, value: scalar(mystery) },
      ],
    };
    let buddy_card = Class {
      hash: GUILD_CARD_CLASS,
      fields: vec![Field { hash: HUNTER_RANK, field_type: 0x08, value: scalar(43) }],
    };
    use crate::payload::Array;
    let buddies = Array {
      member_type: 0x11,
      member_size: 0,
      array_type: 1,
      class_hashes: Some(vec![GUILD_CARD_CLASS]),
      values: vec![crate::payload::ArrayValue::Class(Box::new(buddy_card))],
    };
    SavePayload {
      entries: vec![
        crate::payload::NativeClass {
          native_hash: 0x1,
          class: Class {
            hash: 0x2,
            fields: vec![Field {
              hash: 0x3,
              field_type: 0x11,
              value: FieldValue::Class(Box::new(progress)),
            }],
          },
        },
        crate::payload::NativeClass {
          native_hash: 0x4,
          class: Class {
            hash: 0x5,
            fields: vec![
              Field {
                hash: 0x6,
                field_type: 0x11,
                value: FieldValue::Class(Box::new(guild_card)),
              },
              Field { hash: 0x7, field_type: -1, value: FieldValue::Array(buddies) },
            ],
          },
        },
      ],
    }
  }

  #[test]
  fn reads_and_sets_ranks_in_both_copies_but_not_buddy_cards() {
    let mut payload = ranks_payload(3, 3, 1);

    let before = read_ranks(&payload).expect("read ranks");
    assert_eq!(before, Ranks { hunter: 3, master: 3, mystery_research: 1 });

    let previous = set_ranks(&mut payload, 999, 999, 300).expect("set ranks");
    assert_eq!(previous, Ranks { hunter: 3, master: 3, mystery_research: 1 });

    assert_eq!(read_ranks(&payload), Some(Ranks { hunter: 999, master: 999, mystery_research: 300 }));

    // The self guild card copy is updated alongside.
    let mut cards = Vec::new();
    collect_classes(&payload, GUILD_CARD_CLASS, &mut cards);
    assert_eq!(cards.len(), 2, "self card + one buddy card");
    let card_value = |class: &Class, hash: u32| -> u32 {
      let FieldValue::Scalar { bytes, .. } = &class
        .fields
        .iter()
        .find(|field| field.hash == hash)
        .expect("field")
        .value
      else {
        panic!("scalar");
      };
      u32::from_le_bytes(bytes.as_slice().try_into().expect("4 bytes"))
    };
    assert_eq!(card_value(cards[0], HUNTER_RANK), 999);
    assert_eq!(card_value(cards[0], GUILD_CARD_LABO_LV), 300);
    // The buddy card (nested in an array) is untouched.
    assert_eq!(card_value(cards[1], HUNTER_RANK), 43);
  }

  #[test]
  fn set_ranks_requires_a_guild_card() {
    let mut payload = SavePayload { entries: vec![] };
    let error =
      set_ranks(&mut payload, 99, 99, 9).expect_err("missing progression must fail");
    assert!(
      error.to_string().contains("refusing to write ranks"),
      "unexpected error: {error}"
    );
  }

  #[test]
  fn equipment_transfer_can_be_restricted_to_selected_components() {
    let source = equip_complex_payload(777, 3);
    let mut target = equip_complex_payload(0, 0);

    // Box only: the manager class keeps the target's values.
    let report = transfer_equipment_parts(&mut target, &source, EquipParts {
      equip_box: true,
      equip_manager: false,
      item_my_set: false,
    })
    .expect("box-only transfer should succeed");
    assert_eq!(report.classes.len(), 1);
    assert_eq!(report.classes[0].0, "EquipBox");

    let mut managers = Vec::new();
    collect_classes(&target, EQUIP_MANAGER_SAVE_CLASS, &mut managers);
    assert_eq!(managers.len(), 1);
    // The untouched manager still has the target's worn index.
    let FieldValue::Class(pack) = &managers[0].fields[0].value else {
      panic!("expected nested pack class");
    };
    let FieldValue::Scalar { bytes, .. } = &pack.fields[0].value else {
      panic!("expected scalar");
    };
    assert_eq!(bytes.as_slice(), 0i32.to_le_bytes());

    // Nothing selected: refused.
    let mut fresh = equip_complex_payload(0, 0);
    let error = transfer_equipment_parts(&mut fresh, &source, EquipParts::none())
      .expect_err("empty selection must fail");
    assert!(error.to_string().contains("no equipment components"), "unexpected: {error}");
  }

  fn equip_complex_payload(id_val: u32, worn_index: i32) -> SavePayload {
    use crate::payload::Array;
    let inventory_array = Array {
      member_type: 0x11,
      member_size: 0,
      array_type: 1,
      class_hashes: Some(vec![0xe9f1_0309; 2]),
      values: vec![equip_entry(id_val), equip_entry(0)],
    };
    let equip_box = Class {
      hash: EQUIP_BOX_CLASS,
      fields: vec![Field {
        hash: EQUIP_BOX_WEAPON_ARMOR_LIST,
        field_type: -1,
        value: FieldValue::Array(inventory_array),
      }],
    };
    let equip_pack = Class {
      hash: 0x1cc4_19be, // snow.equip.PlEquipPack
      fields: vec![Field {
        hash: 0x8b35_6c1e, // value placeholder for _InventoryIndexList entry
        field_type: 0x07,
        value: FieldValue::Scalar { size: 4, bytes: worn_index.to_le_bytes().to_vec() },
      }],
    };
    let equip_manager = Class {
      hash: EQUIP_MANAGER_SAVE_CLASS,
      fields: vec![Field {
        hash: 0x6,
        field_type: 0x11,
        value: FieldValue::Class(Box::new(equip_pack)),
      }],
    };
    let item_my_set = Class {
      hash: ITEM_MY_SET_CLASS,
      fields: vec![Field { hash: 0x7, field_type: 0x07, value: scalar(0) }],
    };
    SavePayload {
      entries: vec![crate::payload::NativeClass {
        native_hash: 0x1,
        class: Class {
          hash: 0x2,
          fields: vec![
            Field { hash: 0x5, field_type: 0x11, value: FieldValue::Class(Box::new(equip_box)) },
            Field { hash: 0x8, field_type: 0x11, value: FieldValue::Class(Box::new(equip_manager)) },
            Field { hash: 0x9, field_type: 0x11, value: FieldValue::Class(Box::new(item_my_set)) },
          ],
        },
      }],
    }
  }

  fn equip_entry(id_val: u32) -> crate::payload::ArrayValue {
    use crate::payload::ArrayValue;
    ArrayValue::Class(Box::new(Class {
      hash: 0xe9f1_0309, // snow.data.EquipmentInventoryData
      fields: vec![Field { hash: EQUIP_ID_VAL, field_type: 0x08, value: scalar(id_val) }],
    }))
  }

  #[test]
  fn copy_class_fields_replaces_the_single_instance() {
    let source = equip_complex_payload(777, 3);
    let mut target = equip_complex_payload(0, 0);

    let copied =
      copy_class_fields(&mut target, &source, EQUIP_BOX_CLASS).expect("class copy should succeed");
    assert!(copied > 0);

    let mut boxes = Vec::new();
    collect_classes(&target, EQUIP_BOX_CLASS, &mut boxes);
    assert_eq!(boxes.len(), 1);
    assert_eq!(count_used_equipment_box(boxes[0]), 1);
  }

  fn count_used_equipment_box(box_class: &Class) -> usize {
    for field in &box_class.fields {
      if field.hash == EQUIP_BOX_WEAPON_ARMOR_LIST
        && let FieldValue::Array(array) = &field.value
      {
        return count_used_equipment(array);
      }
    }
    0
  }

  #[test]
  fn copy_class_fields_requires_class_on_both_sides() {
    let source = equip_complex_payload(777, 3);
    let mut target = SavePayload { entries: vec![] };
    let error = copy_class_fields(&mut target, &source, EQUIP_BOX_CLASS).expect_err("must fail");
    assert!(error.to_string().contains("target contains 0"), "unexpected error: {error}");
  }

  fn name_entry(native_hash: u32, name: &str) -> crate::payload::NativeClass {
    let profile = Class {
      hash: 0x4454_1321, // snow.data.ProfileData
      fields: vec![Field {
        hash: HUNTER_NAME_FIELD,
        field_type: 0x0f,
        value: FieldValue::String(name.encode_utf16().collect()),
      }],
    };
    crate::payload::NativeClass {
      native_hash,
      class: Class {
        hash: 0x2,
        fields: vec![Field {
          hash: 0x3,
          field_type: 0x11,
          value: FieldValue::Class(Box::new(profile)),
        }],
      },
    }
  }

  #[test]
  fn reads_hunter_name_from_the_first_profile() {
    // The save carries the hunter profile before buddy lists, which reuse the
    // same name field for buddy names; document order must win.
    let payload = SavePayload {
      entries: vec![
        name_entry(0x10, "原初形态爵银龙"),
        name_entry(0x20, "积极boy"),
      ],
    };
    assert_eq!(read_hunter_name(&payload).as_deref(), Some("原初形态爵银龙"));
    assert_eq!(read_hunter_name(&SavePayload { entries: vec![] }), None);
  }

  #[test]
  fn reads_and_edits_item_box_counts() {
    let mut payload = box_payload(vec![
      item_slot(0x0410_0006, 10),
      item_slot(0x0410_0007, 20),
      item_slot(ITEM_ID_NONE, 0),
    ]);
    let entries = read_item_box(&payload).expect("read box");
    assert_eq!(
      entries,
      vec![
        ItemBoxEntry { id: 0x0410_0006, num: 10 },
        ItemBoxEntry { id: 0x0410_0007, num: 20 },
        ItemBoxEntry { id: ITEM_ID_NONE, num: 0 },
      ]
    );

    // Edit an existing stack, clear another, and add a new item.
    let report =
      set_item_counts(&mut payload, &[(0x0410_0006, 99), (0x0410_0007, 0), (0x0410_0100, 5)])
        .expect("set counts");
    assert_eq!(report.applied, 3);
    assert_eq!(report.filled, 1);

    let entries = read_item_box(&payload).expect("read box");
    assert_eq!(entries[0], ItemBoxEntry { id: 0x0410_0006, num: 99 });
    // The new item filled the first empty slot (the one just cleared).
    assert_eq!(entries[1], ItemBoxEntry { id: 0x0410_0100, num: 5 });
    // The remaining slot stays in the game's empty representation.
    assert_eq!(entries[2], ItemBoxEntry { id: ITEM_ID_NONE, num: 0 });
  }

  #[test]
  fn set_item_counts_refuses_when_box_has_no_room() {
    let mut payload = box_payload(vec![item_slot(0x0410_0006, 10)]);
    let error =
      set_item_counts(&mut payload, &[(0x0410_0200, 1)]).expect_err("no empty slot must fail");
    assert!(error.to_string().contains("full"), "unexpected error: {error}");
  }
  fn loadout_element(name: &str, is_using: bool, indices: &[u32]) -> crate::payload::ArrayValue {
    use crate::payload::Array;
    let index_array = Array {
      member_type: 0x04,
      member_size: 4,
      array_type: 0,
      class_hashes: None,
      values: indices.iter().map(|index| ArrayValue::Scalar(index.to_le_bytes().to_vec())).collect(),
    };
    ArrayValue::Class(Box::new(Class {
      hash: 0xd897_238a, // snow.equip.PlEquipMySetData
      fields: vec![
        Field { hash: EQUIP_MYSET_IS_USING, field_type: 0x01, value: FieldValue::Scalar { size: 1, bytes: vec![u8::from(is_using)] } },
        Field { hash: EQUIP_MYSET_NAME, field_type: 0x0f, value: FieldValue::String(name.encode_utf16().collect()) },
        Field { hash: EQUIP_MYSET_INVENTORY_INDEX_LIST, field_type: -1, value: FieldValue::Array(index_array) },
      ],
    }))
  }

  /// EquipDataManager with a 4-slot loadout list + an 8-slot equipment box.
  fn loadout_payload(loadouts: Vec<crate::payload::ArrayValue>) -> SavePayload {
    use crate::payload::Array;
    let mut loadout_values = loadouts;
    while loadout_values.len() < 4 {
      loadout_values.push(loadout_element("(free)", false, &[]));
    }
    let loadout_list = Array {
      member_type: 0x11,
      member_size: 0,
      array_type: 1,
      class_hashes: Some(vec![0xd897_238a; loadout_values.len()]),
      values: loadout_values,
    };
    let manager = Class {
      hash: EQUIP_MANAGER_SAVE_CLASS,
      fields: vec![Field {
        hash: EQUIP_MY_SET_LIST,
        field_type: -1,
        value: FieldValue::Array(loadout_list),
      }],
    };
    let mut box_values = Vec::new();
    for index in 0..8 {
      box_values.push(equip_entry(1000 + index as u32));
    }
    let box_list = Array {
      member_type: 0x11,
      member_size: 0,
      array_type: 1,
      class_hashes: Some(vec![0xe9f1_0309; box_values.len()]),
      values: box_values,
    };
    let equip_box = Class {
      hash: EQUIP_BOX_CLASS,
      fields: vec![Field {
        hash: EQUIP_BOX_WEAPON_ARMOR_LIST,
        field_type: -1,
        value: FieldValue::Array(box_list),
      }],
    };
    SavePayload {
      entries: vec![crate::payload::NativeClass {
        native_hash: 0x1,
        class: Class {
          hash: 0x2,
          fields: vec![
            Field { hash: 0x5, field_type: 0x11, value: FieldValue::Class(Box::new(equip_box)) },
            Field { hash: 0x8, field_type: 0x11, value: FieldValue::Class(Box::new(manager)) },
          ],
        },
      }],
    }
  }

  #[test]
  fn reads_equip_loadouts_with_names_and_indices() {
    let payload = loadout_payload(vec![
      loadout_element("魔狂化双刀雷", true, &[0, 1, 2, 3, 4, 5, 6, 7]),
      loadout_element("(free)", false, &[]),
    ]);
    let loadouts = read_equip_loadouts(&payload);
    assert_eq!(loadouts.len(), 4);
    assert_eq!(loadouts[0].index, 0);
    assert_eq!(loadouts[0].name, "魔狂化双刀雷");
    assert!(loadouts[0].is_using);
    assert_eq!(loadouts[0].inventory_indices, vec![0, 1, 2, 3, 4, 5, 6, 7]);
    assert!(!loadouts[1].is_using);
  }

  #[test]
  fn copies_loadout_with_referenced_equipment_to_a_slot() {
    let source = loadout_payload(vec![loadout_element("魔狂化双刀雷", true, &[0, 1, 2])]);
    let mut target = loadout_payload(vec![loadout_element("旧组合", true, &[5, 6])]);

    let slot =
      copy_equip_loadout(&mut target, &source, 0, Some(2), true).expect("loadout copy should succeed");
    assert_eq!(slot, 2);

    let loadouts = read_equip_loadouts(&target);
    assert_eq!(loadouts[2].name, "魔狂化双刀雷");
    assert!(loadouts[2].is_using);
    assert_eq!(loadouts[2].inventory_indices, vec![0, 1, 2]);

    // Referenced box pieces were copied at the same indices.
    let box_list = equip_armor_list(&target).expect("box");
    let piece_id = |index: usize| -> u32 {
      let crate::payload::ArrayValue::Class(class) = &box_list.values[index] else {
        panic!("class");
      };
      let FieldValue::Scalar { bytes, .. } = &class
        .fields
        .iter()
        .find(|field| field.hash == EQUIP_ID_VAL)
        .expect("id field")
        .value
      else {
        panic!("scalar");
      };
      u32::from_le_bytes(bytes.as_slice().try_into().expect("4 bytes"))
    };
    assert_eq!(piece_id(0), 1000); // copied from source
    assert_eq!(piece_id(1), 1001);
    assert_eq!(piece_id(2), 1002);
    assert_eq!(piece_id(5), 1005); // untouched target slot keeps its piece
    // The overwritten target slot 2 kept its own register contents nowhere —
    // the old register at slot 0 is still the target's.
    assert_eq!(loadouts[0].name, "旧组合");
  }

  #[test]
  fn copies_loadout_appends_to_the_first_free_slot() {
    let source = loadout_payload(vec![loadout_element("新组合", true, &[3])]);
    let mut target = loadout_payload(vec![
      loadout_element("占用一", true, &[]),
      loadout_element("(free)", false, &[]),
      loadout_element("占用二", true, &[]),
    ]);
    let slot = copy_equip_loadout(&mut target, &source, 0, None, false).expect("append should succeed");
    assert_eq!(slot, 1);
    let loadouts = read_equip_loadouts(&target);
    assert_eq!(loadouts[1].name, "新组合");
    assert!(loadouts[1].is_using);
  }

  #[test]
  fn copies_a_single_equipment_piece_between_slots() {
    let source = loadout_payload(vec![]);
    let mut target = loadout_payload(vec![]);
    copy_equip_piece(&mut target, &source, 7, 0).expect("piece copy should succeed");
    let box_list = equip_armor_list(&target).expect("box");
    let crate::payload::ArrayValue::Class(class) = &box_list.values[0] else {
      panic!("class");
    };
    let FieldValue::Scalar { bytes, .. } = &class
      .fields
      .iter()
      .find(|field| field.hash == EQUIP_ID_VAL)
      .expect("id field")
      .value
    else {
      panic!("scalar");
    };
    assert_eq!(u32::from_le_bytes(bytes.as_slice().try_into().expect("4 bytes")), 1007);
  }

  #[test]
  fn copy_equip_piece_rejects_out_of_range_slots() {
    let source = loadout_payload(vec![]);
    let mut target = loadout_payload(vec![]);
    let error =
      copy_equip_piece(&mut target, &source, 99, 0).expect_err("source range must be enforced");
    assert!(error.to_string().contains("out of range"), "unexpected: {error}");
    let error =
      copy_equip_piece(&mut target, &source, 0, 99).expect_err("target range must be enforced");
    assert!(error.to_string().contains("out of range"), "unexpected: {error}");
  }

}
