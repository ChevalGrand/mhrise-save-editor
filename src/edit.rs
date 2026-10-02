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
  let classes = vec![
    ("EquipBox", copy_class_fields(target, source, EQUIP_BOX_CLASS)?),
    ("EquipManager", copy_class_fields(target, source, EQUIP_MANAGER_SAVE_CLASS)?),
    ("ItemMySet", copy_class_fields(target, source, ITEM_MY_SET_CLASS)?),
  ];

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
    SavePayload {
      entries: vec![crate::payload::NativeClass {
        native_hash: 0x1,
        class: Class {
          hash: 0x2,
          fields: vec![
            Field { hash: 0x5, field_type: 0x11, value: FieldValue::Class(Box::new(equip_box)) },
            Field { hash: 0x6, field_type: 0x11, value: FieldValue::Class(Box::new(equip_pack)) },
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
}
