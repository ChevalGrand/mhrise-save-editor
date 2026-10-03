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
/// `snow.data.ItemMySet` — registered item pouch loadouts.
pub const ITEM_MY_SET_CLASS: u32 = 0x8923_cd68;
/// `snow.data.EquipmentInventoryData._IdVal` — the equipment piece id.
pub const EQUIP_ID_VAL: u32 = 0x6e86_4182;
/// `snow.data.EquipBox._WeaponArmorInventoryList`
pub const EQUIP_BOX_WEAPON_ARMOR_LIST: u32 = 0xa097_349e;

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
}
