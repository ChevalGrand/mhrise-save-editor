//! Structural diff between two payload trees.
//!
//! Trees are compared by structure (classes by field hash, arrays by index);
//! every differing scalar becomes a [`Change`] with a hash path. For reverse
//! engineering, changes are aggregated by their index-free path so that a
//! single game field touched in many array slots (item quantities, for
//! example) shows up as one high-count aggregate.

use std::collections::BTreeMap;

use crate::payload::{Array, ArrayValue, Class, Field, FieldValue, SavePayload};

const MAX_SAMPLE_VALUES: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
  ValueChanged,
  Added,
  Removed,
}

impl ChangeKind {
  pub fn label(self) -> &'static str {
    match self {
      ChangeKind::ValueChanged => "changed",
      ChangeKind::Added => "added",
      ChangeKind::Removed => "removed",
    }
  }
}

#[derive(Debug, Clone)]
pub struct Change {
  /// Hash path of the change, e.g. `@85e904c1/5766f30b.0a960102[3]`.
  pub path: String,
  /// The same path with all array indices stripped, for aggregation.
  pub aggregate_path: String,
  pub kind: ChangeKind,
  pub before: Option<String>,
  pub after: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct DiffReport {
  pub changes: Vec<Change>,
  /// True when the change list was capped; the total count stays complete.
  pub changes_truncated: bool,
  pub total_changes: usize,
}

#[derive(Debug, Clone)]
pub struct Aggregate {
  pub aggregate_path: String,
  pub changed: usize,
  pub added: usize,
  pub removed: usize,
  pub samples: Vec<(Option<String>, Option<String>)>,
}

pub fn diff_payloads(before: &SavePayload, after: &SavePayload, max_changes: usize) -> DiffReport {
  let mut state = DiffState::default();
  let before_entries: BTreeMap<u32, &crate::payload::NativeClass> =
    before.entries.iter().map(|entry| (entry.native_hash, entry)).collect();
  let after_entries: BTreeMap<u32, &crate::payload::NativeClass> =
    after.entries.iter().map(|entry| (entry.native_hash, entry)).collect();

  let mut hashes: Vec<u32> = before_entries.keys().chain(after_entries.keys()).copied().collect();
  hashes.sort_unstable();
  hashes.dedup();
  for native_hash in hashes {
    let entry_path = format!("@{native_hash:08x}");
    match (before_entries.get(&native_hash), after_entries.get(&native_hash)) {
      (Some(before_entry), Some(after_entry)) => {
        diff_class(&before_entry.class, &after_entry.class, &entry_path, &mut state);
      }
      (Some(before_entry), None) => state.push_removed_class(&before_entry.class, &entry_path),
      (None, Some(after_entry)) => state.push_added_class(&after_entry.class, &entry_path),
      (None, None) => unreachable!("hash came from one of the maps"),
    }
  }

  let total_changes = state.changes.len();
  let mut changes = state.changes;
  let changes_truncated = changes.len() > max_changes;
  changes.truncate(max_changes);
  DiffReport { changes, changes_truncated, total_changes }
}

/// Groups changes by their index-free path, sorted by total change count.
pub fn aggregate_changes(changes: &[Change]) -> Vec<Aggregate> {
  let mut groups: BTreeMap<String, Aggregate> = BTreeMap::new();
  for change in changes {
    let group = groups.entry(change.aggregate_path.clone()).or_insert_with(|| Aggregate {
      aggregate_path: change.aggregate_path.clone(),
      changed: 0,
      added: 0,
      removed: 0,
      samples: Vec::new(),
    });
    match change.kind {
      ChangeKind::ValueChanged => group.changed += 1,
      ChangeKind::Added => group.added += 1,
      ChangeKind::Removed => group.removed += 1,
    }
    if group.samples.len() < MAX_SAMPLE_VALUES {
      group.samples.push((change.before.clone(), change.after.clone()));
    }
  }
  let mut aggregates: Vec<Aggregate> = groups.into_values().collect();
  aggregates.sort_by_key(|group| group.changed + group.added + group.removed);
  aggregates.reverse();
  aggregates
}

#[derive(Default)]
struct DiffState {
  changes: Vec<Change>,
}

impl DiffState {
  fn push(
    &mut self,
    path: String,
    kind: ChangeKind,
    before: Option<String>,
    after: Option<String>,
  ) {
    let aggregate_path = strip_indices(&path);
    self.changes.push(Change { path, aggregate_path, kind, before, after });
  }

  fn push_removed_class(&mut self, class: &Class, path: &str) {
    self.push(
      format!("{path}/removed-class"),
      ChangeKind::Removed,
      Some(format!("class {:08x} ({} fields)", class.hash, class.fields.len())),
      None,
    );
  }

  fn push_added_class(&mut self, class: &Class, path: &str) {
    self.push(
      format!("{path}/added-class"),
      ChangeKind::Added,
      None,
      Some(format!("class {:08x} ({} fields)", class.hash, class.fields.len())),
    );
  }
}

fn diff_class(before: &Class, after: &Class, path: &str, state: &mut DiffState) {
  let class_path = format!("{path}/{:08x}", after.hash);
  let before_fields: BTreeMap<u32, &Field> =
    before.fields.iter().map(|field| (field.hash, field)).collect();
  let after_fields: BTreeMap<u32, &Field> =
    after.fields.iter().map(|field| (field.hash, field)).collect();

  let mut hashes: Vec<u32> = before_fields.keys().chain(after_fields.keys()).copied().collect();
  hashes.sort_unstable();
  hashes.dedup();
  for field_hash in hashes {
    let field_path = format!("{class_path}.{field_hash:08x}");
    match (before_fields.get(&field_hash), after_fields.get(&field_hash)) {
      (Some(before_field), Some(after_field)) => {
        diff_values(&before_field.value, &after_field.value, &field_path, state);
      }
      (Some(before_field), None) => {
        state.push(field_path, ChangeKind::Removed, Some(describe_value(&before_field.value)), None)
      }
      (None, Some(after_field)) => {
        state.push(field_path, ChangeKind::Added, None, Some(describe_value(&after_field.value)))
      }
      (None, None) => unreachable!("hash came from one of the maps"),
    }
  }
}

fn diff_values(before: &FieldValue, after: &FieldValue, path: &str, state: &mut DiffState) {
  match (before, after) {
    (
      FieldValue::Scalar { size: before_size, bytes: before_bytes },
      FieldValue::Scalar { size: after_size, bytes: after_bytes },
    ) => {
      if before_size != after_size || before_bytes != after_bytes {
        state.push(
          path.to_owned(),
          ChangeKind::ValueChanged,
          Some(describe_scalar(*before_size, before_bytes)),
          Some(describe_scalar(*after_size, after_bytes)),
        );
      }
    }
    (FieldValue::String(before_units), FieldValue::String(after_units)) => {
      if before_units != after_units {
        state.push(
          path.to_owned(),
          ChangeKind::ValueChanged,
          Some(describe_string(before_units)),
          Some(describe_string(after_units)),
        );
      }
    }
    (FieldValue::Class(before_class), FieldValue::Class(after_class)) => {
      diff_class(before_class, after_class, path, state);
    }
    (FieldValue::Array(before_array), FieldValue::Array(after_array)) => {
      diff_arrays(before_array, after_array, path, state);
    }
    (before_value, after_value) => {
      state.push(
        path.to_owned(),
        ChangeKind::ValueChanged,
        Some(describe_value(before_value)),
        Some(describe_value(after_value)),
      );
    }
  }
}

fn diff_arrays(before: &Array, after: &Array, path: &str, state: &mut DiffState) {
  if before.array_type != after.array_type {
    state.push(
      path.to_owned(),
      ChangeKind::ValueChanged,
      Some(format!("array type {}", before.array_type)),
      Some(format!("array type {}", after.array_type)),
    );
    return;
  }

  let common = before.values.len().min(after.values.len());
  for index in 0..common {
    let element_path = format!("{path}[{index}]");
    diff_array_values(&before.values[index], &after.values[index], &element_path, state);
  }
  for index in common..before.values.len() {
    state.push(
      format!("{path}[{index}]"),
      ChangeKind::Removed,
      Some(describe_array_value(&before.values[index])),
      None,
    );
  }
  for index in common..after.values.len() {
    state.push(
      format!("{path}[{index}]"),
      ChangeKind::Added,
      None,
      Some(describe_array_value(&after.values[index])),
    );
  }
}

fn diff_array_values(before: &ArrayValue, after: &ArrayValue, path: &str, state: &mut DiffState) {
  match (before, after) {
    (ArrayValue::Scalar(before_bytes), ArrayValue::Scalar(after_bytes)) => {
      if before_bytes != after_bytes {
        state.push(
          path.to_owned(),
          ChangeKind::ValueChanged,
          Some(describe_scalar(before_bytes.len() as u32, before_bytes)),
          Some(describe_scalar(after_bytes.len() as u32, after_bytes)),
        );
      }
    }
    (ArrayValue::String(before_units), ArrayValue::String(after_units)) => {
      if before_units != after_units {
        state.push(
          path.to_owned(),
          ChangeKind::ValueChanged,
          Some(describe_string(before_units)),
          Some(describe_string(after_units)),
        );
      }
    }
    (ArrayValue::Class(before_class), ArrayValue::Class(after_class)) => {
      diff_class(before_class, after_class, path, state);
    }
    (ArrayValue::Array(before_array), ArrayValue::Array(after_array)) => {
      diff_arrays(before_array, after_array, path, state);
    }
    (before_value, after_value) => {
      state.push(
        path.to_owned(),
        ChangeKind::ValueChanged,
        Some(describe_array_value(before_value)),
        Some(describe_array_value(after_value)),
      );
    }
  }
}

/// Removes every `[...]` segment so that array elements aggregate together.
fn strip_indices(path: &str) -> String {
  let mut out = String::with_capacity(path.len());
  let mut depth = 0usize;
  for character in path.chars() {
    match character {
      '[' => depth += 1,
      ']' => depth = depth.saturating_sub(1),
      c if depth == 0 => out.push(c),
      _ => {}
    }
  }
  out
}

fn describe_value(value: &FieldValue) -> String {
  match value {
    FieldValue::Scalar { size, bytes } => describe_scalar(*size, bytes),
    FieldValue::String(units) => describe_string(units),
    FieldValue::Class(class) => format!("class {:08x} ({} fields)", class.hash, class.fields.len()),
    FieldValue::Array(array) => format!("array of {} elements", array.values.len()),
  }
}

fn describe_array_value(value: &ArrayValue) -> String {
  match value {
    ArrayValue::Scalar(bytes) => describe_scalar(bytes.len() as u32, bytes),
    ArrayValue::String(units) => describe_string(units),
    ArrayValue::Class(class) => format!("class {:08x} ({} fields)", class.hash, class.fields.len()),
    ArrayValue::Array(array) => format!("array of {} elements", array.values.len()),
  }
}

fn describe_scalar(size: u32, bytes: &[u8]) -> String {
  let mut text = format!("{size}B {bytes:02x?}");
  if let Some(value) = scalar_uint(size, bytes) {
    text.push_str(&format!(" = {value}"));
  }
  text
}

fn describe_string(units: &[u16]) -> String {
  format!("string {:?}", String::from_utf16_lossy(units))
}

fn scalar_uint(size: u32, bytes: &[u8]) -> Option<u64> {
  match size {
    1 => bytes.try_into().ok().map(u8::from_le_bytes).map(u64::from),
    2 => bytes.try_into().ok().map(u16::from_le_bytes).map(u64::from),
    4 => bytes.try_into().ok().map(u32::from_le_bytes).map(u64::from),
    8 => bytes.try_into().ok().map(u64::from_le_bytes),
    _ => None,
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::payload::NativeClass;

  fn payload_with(field_hash: u32, value: FieldValue) -> SavePayload {
    SavePayload {
      entries: vec![NativeClass {
        native_hash: 0x1,
        class: Class {
          hash: 0x2,
          fields: vec![Field { hash: field_hash, field_type: 0x08, value }],
        },
      }],
    }
  }

  fn scalar(value: u32) -> FieldValue {
    FieldValue::Scalar { size: 4, bytes: value.to_le_bytes().to_vec() }
  }

  fn u32_array(values: Vec<u32>) -> FieldValue {
    FieldValue::Array(Array {
      member_type: 0x08,
      member_size: 4,
      array_type: 0,
      class_hashes: None,
      values: values.into_iter().map(|v| ArrayValue::Scalar(v.to_le_bytes().to_vec())).collect(),
    })
  }

  #[test]
  fn reports_changed_scalar_with_path() {
    let before = payload_with(0xaaaa, scalar(100));
    let after = payload_with(0xaaaa, scalar(9999));
    let report = diff_payloads(&before, &after, 100);
    assert_eq!(report.total_changes, 1);
    let change = &report.changes[0];
    assert_eq!(change.path, "@00000001/00000002.0000aaaa");
    assert_eq!(change.before.as_deref(), Some("4B [64, 00, 00, 00] = 100"));
    assert_eq!(change.after.as_deref(), Some("4B [0f, 27, 00, 00] = 9999"));
  }

  #[test]
  fn reports_array_growth_as_added_elements() {
    let before = payload_with(0xbbbb, u32_array(vec![1, 2, 3]));
    let after = payload_with(0xbbbb, u32_array(vec![1, 2, 3, 4, 5]));
    let report = diff_payloads(&before, &after, 100);
    assert_eq!(report.total_changes, 2);
    assert!(report.changes.iter().all(|change| change.kind == ChangeKind::Added));
    assert_eq!(report.changes[0].path, "@00000001/00000002.0000bbbb[3]");
    let aggregates = aggregate_changes(&report.changes);
    assert_eq!(aggregates.len(), 1);
    assert_eq!(aggregates[0].aggregate_path, "@00000001/00000002.0000bbbb");
    assert_eq!(aggregates[0].added, 2);
  }

  #[test]
  fn aggregation_groups_indexed_changes() {
    let before = payload_with(0xcccc, u32_array(vec![10, 10, 10]));
    let after = payload_with(0xcccc, u32_array(vec![11, 10, 12]));
    let report = diff_payloads(&before, &after, 100);
    assert_eq!(report.total_changes, 2);
    let aggregates = aggregate_changes(&report.changes);
    assert_eq!(aggregates.len(), 1);
    assert_eq!(aggregates[0].changed, 2);
    assert_eq!(aggregates[0].samples.len(), 2);
  }

  #[test]
  fn truncation_keeps_totals_accurate() {
    // 0 -> 0 is not a change, so 99 of the 100 elements actually differ.
    let before = payload_with(0xdddd, u32_array((0..100).collect()));
    let after = payload_with(0xdddd, u32_array((0..100).map(|v| v * 2).collect()));
    let report = diff_payloads(&before, &after, 10);
    assert_eq!(report.total_changes, 99);
    assert!(report.changes_truncated);
    assert_eq!(report.changes.len(), 10);
  }
}
