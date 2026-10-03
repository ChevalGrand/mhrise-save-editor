//! eframe/egui front end for the save editor.
//!
//! Model: open a save once (background thread — decryption takes seconds),
//! edit values in memory (instant), then write the result to a NEW file
//! (`保存为新文件`). The selected save itself is never modified; the user
//! swaps the output file in with the game closed.

use std::{
  fs,
  path::{Path, PathBuf},
  sync::mpsc::{self, Receiver, TryRecvError},
  sync::OnceLock,
  thread,
  time::Duration,
};

use eframe::egui;
use egui::Grid;

use crate::{container::SteamSave, edit, format::parse_header};

const TAB_INFO: usize = 0;
const TAB_PLAYER: usize = 1;
const TAB_ITEMBOX: usize = 2;
const TAB_EQUIP: usize = 3;
const TAB_TRANSFER: usize = 4;
const TAB_PROGRESS: usize = 5;
const TAB_BACKUP: usize = 6;
const TAB_HELP: usize = 7;

/// Loads a single-face CJK .ttf font. egui's text rasterizer (ab_glyph) does
/// NOT support .ttc collections, which is why msyh.ttc/simsun.ttc cannot be
/// used directly. Returns a status line for the on-screen log.
fn install_cjk_font(ctx: &egui::Context) -> String {
  for (path, label) in [
    ("C:/Windows/Fonts/Deng.ttf", "等线 (Deng.ttf)"),
    ("C:/Windows/Fonts/simhei.ttf", "黑体 (simhei.ttf)"),
    ("C:/Windows/Fonts/Dengb.ttf", "等线粗体 (Dengb.ttf)"),
    ("C:/Windows/Fonts/STXIHEI.TTF", "华文细黑 (STXIHEI.TTF)"),
  ] {
    let Ok(bytes) = fs::read(path) else {
      continue;
    };
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert("cjk".to_owned(), egui::FontData::from_owned(bytes).into());
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
      fonts.families.entry(family).or_default().insert(0, "cjk".to_owned());
    }
    ctx.set_fonts(fonts);
    return format!("字体: 已加载 {label}");
  }
  "字体: 未找到系统中文字体,中文将显示为方框".to_owned()
}

// ---------------------------------------------------------------------------
// Opened document
// ---------------------------------------------------------------------------

struct OpenedSave {
  path: PathBuf,
  document: SteamSave,
  hunter_name: String,
  box_items: Vec<(u32, u32)>,
}

impl OpenedSave {
  fn open(path: PathBuf, steamid64: u64) -> anyhow::Result<Self> {
    let document = SteamSave::open_path(&path, steamid64)?;
    let hunter_name = edit::read_hunter_name(document.payload()).unwrap_or_default();
    let box_items = edit::read_item_box(document.payload())?
      .into_iter()
      .map(|entry| (entry.id, entry.num))
      .collect();
    Ok(Self { path, document, hunter_name, box_items })
  }

  fn summary(&self) -> String {
    let hunter = if self.hunter_name.is_empty() { "(未知名)" } else { &self.hunter_name };
    format!("{hunter} · Curve {}", self.document.curve_index())
  }
}

enum WorkerEvent {
  Opened(Result<Box<OpenedSave>, String>, bool),
  /// Wrote the document; the second element is the automatic backup location
  /// for in-place saves.
  Saved(Result<(PathBuf, Option<PathBuf>), String>),
  /// A background operation that only produces a status line (backup/restore).
  Report(Result<String, String>),
}

// ---------------------------------------------------------------------------
// Application state
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct GuiApp {
  target_path: String,
  source_path: String,
  steamid64: String,
  discovered: Vec<(PathBuf, String, Option<u32>)>,
  target: Option<Box<OpenedSave>>,
  source: Option<Box<OpenedSave>>,
  dirty: bool,
  reopen_clicked_once: bool,
  status_error: Option<String>,
  money_input: String,
  points_input: String,
  total_input: String,
  hunter_rank_input: String,
  master_rank_input: String,
  mystery_rank_input: String,
  box_filter: String,
  box_edits: Vec<String>,
  busy: bool,
  busy_text: String,
  log: String,
  receiver: Option<Receiver<WorkerEvent>>,
  tab: usize,
  /// Item-category toggles for the transfer filter, parallel to
  /// [`ITEM_CATEGORY_GROUPS`].
  xfer_cats: Vec<bool>,
  xfer_equip_box: bool,
  xfer_equip_manager: bool,
  xfer_item_my_set: bool,
  /// Backup awaiting a second confirming click on 还原.
  restore_confirm: Option<PathBuf>,
  /// Point-accumulator inputs on the 进度与解禁 tab (HR/MR/anomaly research).
  hr_point_input: String,
  mr_point_input: String,
  mystery_point_input: String,
  /// Unlock-flag toggles on the 进度与解禁 tab, parallel to
  /// [`edit::GUILD_FLAGS`].
  guild_flags: Vec<bool>,
  /// Transfer toggle: copy ProgressSaveData (ranks + points).
  xfer_progress: bool,
  /// 装备组合 tab state: selected source register, target slot text
  /// (empty = append to the first free slot), and the single-piece inputs.
  equip_source_combo: usize,
  equip_target_slot_text: String,
  equip_with_pieces: bool,
  equip_piece_source_text: String,
  equip_piece_target_text: String,
}

/// (label, item-table categories) groups for the item-box transfer filter.
/// Ids missing from the name table follow the last ("其它/未知") toggle.
const ITEM_CATEGORY_GROUPS: &[(&str, &[&str])] = &[
  ("消耗品", &["Consume"]),
  ("素材", &["Material", "OffcutsMaterial"]),
  ("弹药·瓶", &["Bullet", "Bottle"]),
  ("换金·古董", &["PayOff", "CarryPayOff", "Antique"]),
  ("其它/工具/未知", &["Tool"]),
];

impl GuiApp {
  pub fn new(creation_context: &eframe::CreationContext) -> Self {
    let font_status = install_cjk_font(&creation_context.egui_ctx);
    let mut app = Self {
      xfer_cats: vec![true; ITEM_CATEGORY_GROUPS.len()],
      xfer_equip_box: true,
      xfer_equip_manager: true,
      xfer_item_my_set: true,
      guild_flags: vec![false; edit::GUILD_FLAGS.len()],
      ..Self::default()
    };
    app.log_line(&font_status);
    app.discovered = discover_steam_slot_files();
    if let Some((_, _, Some(account))) = app.discovered.first() {
      app.steamid64 = steamid64_from_account(*account).to_string();
    }
    if app.discovered.is_empty() {
      app.log_line("自动查找: 未在本机找到 MHR 存档目录,请手动选择存档文件 (见“帮助”页)");
    } else {
      app.log_line(&format!(
        "自动查找: 找到 {} 个角色存档,可在“快速选择”中选取",
        app.discovered.len()
      ));
    }
    app
  }

  /// Re-reads ranks/points/flag inputs from the opened document.
  fn refresh_progress_inputs(&mut self) {
    let Some(opened) = self.target.as_ref() else {
      return;
    };
    let payload = opened.document.payload();
    if let Some(ranks) = edit::read_ranks(payload) {
      self.hunter_rank_input = ranks.hunter.to_string();
      self.master_rank_input = ranks.master.to_string();
      self.mystery_rank_input = ranks.mystery_research.to_string();
    }
    let points = |hash: u32| -> String {
      edit::read_scalar(payload, edit::PROGRESS_SAVE_CLASS, hash)
        .map(|value| value.to_string())
        .unwrap_or_default()
    };
    self.hr_point_input = points(edit::HUNTER_RANK_POINT);
    self.mr_point_input = points(edit::MASTER_RANK_POINT);
    self.mystery_point_input = points(edit::MYSTERY_RESEARCH_POINT);
    if let Some(flags) = edit::read_guild_flags(payload) {
      self.guild_flags = flags.into_iter().map(|(_, value)| value).collect();
    }
  }

  fn log_line(&mut self, text: &str) {
    self.log.push_str(text);
    self.log.push('\n');
  }

  fn parsed_steamid64(&self) -> Result<u64, &'static str> {
    self.steamid64.trim().parse::<u64>().map_err(|_| "SteamID64 必须是纯数字")
  }

  fn spawn(&mut self, context: &egui::Context, task: WorkerTask) {
    let (sender, receiver) = mpsc::channel();
    self.receiver = Some(receiver);
    self.busy = true;
    self.busy_text = task.describe();
    self.log_line(&format!("▶ {}", task.describe()));
    let context = context.clone();
    thread::spawn(move || {
      let event = task.run();
      let _ = sender.send(event);
      context.request_repaint();
    });
  }

  /// Applies the player-data edits (money / points / lifetime counter) that
  /// differ from the opened document. Runs on the UI thread: pure tree walks.
  fn apply_player_edits(&mut self) -> Result<(), String> {
    let Some(opened) = self.target.as_mut() else {
      return Err("尚未打开目标存档".to_owned());
    };
    let (current_money, current_total) = edit::read_money(opened.document.payload())
      .ok_or_else(|| "未找到金钱字段 (schema 不符)".to_owned())?;
    let current_points = edit::read_points(opened.document.payload())
      .ok_or_else(|| "未找到点数字段 (schema 不符)".to_owned())?;

    let mut applied = 0usize;
    if let Ok(value) = self.money_input.trim().parse::<u32>()
      && value != current_money
    {
      edit::set_scalar(
        opened.document.payload_mut(),
        edit::HAND_MONEY_CLASS,
        edit::HAND_MONEY_VALUE,
        value,
        "_Value",
      )
      .map_err(|error| error.to_string())?;
      applied += 1;
    }
    if let Ok(value) = self.total_input.trim().parse::<u32>()
      && value != current_total
    {
      edit::set_scalar(
        opened.document.payload_mut(),
        edit::HAND_MONEY_CLASS,
        edit::HAND_MONEY_TOTAL_ADDED,
        value,
        "_TotalAddedValue",
      )
      .map_err(|error| error.to_string())?;
      applied += 1;
    }
    if let Ok(value) = self.points_input.trim().parse::<u32>()
      && value != current_points
    {
      edit::set_scalar(
        opened.document.payload_mut(),
        edit::VILLAGE_POINT_CLASS,
        edit::VILLAGE_POINT_VALUE,
        value,
        "_Point",
      )
      .map_err(|error| error.to_string())?;
      applied += 1;
    }

    // Ranks: only apply when the input parses and differs; the ranges are the
    // game's caps (HR/MR 999, anomaly research 300).
    let rank_parse = |text: &str| -> Result<Option<u32>, String> {
      let trimmed = text.trim();
      if trimmed.is_empty() {
        return Ok(None);
      }
      let value = trimmed.parse::<u32>().map_err(|_| format!("等级必须是数字: {text:?}"))?;
      Ok(Some(value))
    };
    let hunter_rank = rank_parse(&self.hunter_rank_input)?;
    let master_rank = rank_parse(&self.master_rank_input)?;
    let mystery_rank = rank_parse(&self.mystery_rank_input)?;
    for (name, value, cap) in [
      ("猎人等级", hunter_rank, 999),
      ("大师等级", master_rank, 999),
      ("怪异研究等级", mystery_rank, 300),
    ] {
      if let Some(value) = value
        && (value == 0 || value > cap)
      {
        return Err(format!("{name} 超出范围 (1-{cap}): {value}"));
      }
    }
    if hunter_rank.is_some() || master_rank.is_some() || mystery_rank.is_some() {
      let current = edit::read_ranks(opened.document.payload())
        .ok_or_else(|| "未找到等级字段 (schema 不符)".to_owned())?;
      let changed = |input: Option<u32>, current_value: u32| input.is_some_and(|v| v != current_value);
      if changed(hunter_rank, current.hunter)
        || changed(master_rank, current.master)
        || changed(mystery_rank, current.mystery_research)
      {
        let hunter = hunter_rank.unwrap_or(current.hunter);
        let master = master_rank.unwrap_or(current.master);
        let mystery = mystery_rank.unwrap_or(current.mystery_research);
        edit::set_ranks(opened.document.payload_mut(), hunter, master, mystery)
          .map_err(|error| error.to_string())?;
        applied += 1;
      }
    }

    if applied == 0 {
      return Err("没有可应用的修改 (数值未变化或输入为空)".to_owned());
    }
    // Refresh the input boxes from the document so they show the new values.
    let (money, total) = edit::read_money(opened.document.payload())
      .ok_or_else(|| "未找到金钱字段 (schema 不符)".to_owned())?;
    let points = edit::read_points(opened.document.payload())
      .ok_or_else(|| "未找到点数字段 (schema 不符)".to_owned())?;
    self.money_input = money.to_string();
    self.total_input = total.to_string();
    self.points_input = points.to_string();
    if let Some(ranks) = edit::read_ranks(opened.document.payload()) {
      self.hunter_rank_input = ranks.hunter.to_string();
      self.master_rank_input = ranks.master.to_string();
      self.mystery_rank_input = ranks.mystery_research.to_string();
    }
    self.dirty = true;
    Ok(())
  }

  fn pending_item_changes(&self) -> Vec<(u32, u32)> {
    let Some(opened) = self.target.as_ref() else {
      return Vec::new();
    };
    let mut changes = Vec::new();
    if opened.box_items.is_empty() || opened.box_items.len() != self.box_edits.len() {
      return changes;
    }
    for (index, (id, num)) in opened.box_items.iter().enumerate() {
      let text = self.box_edits[index].trim();
      if text.is_empty() {
        continue;
      }
      if let Ok(new_value) = text.parse::<u32>()
        && new_value != *num
      {
        changes.push((*id, new_value));
      }
    }
    changes
  }

  fn apply_box_edits(&mut self) -> Result<String, String> {
    let changes = self.pending_item_changes();
    if changes.is_empty() {
      return Err("没有可应用的修改 (数量未变化或输入为空)".to_owned());
    }
    let Some(opened) = self.target.as_mut() else {
      return Err("尚未打开目标存档".to_owned());
    };
    let report = edit::set_item_counts(opened.document.payload_mut(), &changes)
      .map_err(|error| error.to_string())?;
    opened.box_items = edit::read_item_box(opened.document.payload())
      .map_err(|error| error.to_string())?
      .into_iter()
      .map(|entry| (entry.id, entry.num))
      .collect();
    self.box_edits = vec![String::new(); opened.box_items.len()];
    self.dirty = true;
    Ok(format!("{} 处更新,{} 个新物品填入空槽", report.applied, report.filled))
  }

  /// Merge-transfer only the source item slots whose category group is
  /// enabled; other slots keep the target's contents.
  fn transfer_items_filtered(&mut self) -> Result<String, String> {
    let Some(source) = self.source.as_ref() else {
      return Err("尚未打开来源存档".to_owned());
    };
    let groups: Vec<&[&str]> = ITEM_CATEGORY_GROUPS
      .iter()
      .zip(&self.xfer_cats)
      .filter(|(_, on)| **on)
      .map(|((_, cats), _)| *cats)
      .collect();
    let other_allowed = *self.xfer_cats.last().unwrap_or(&false);
    let keep = move |id: u32| match crate::items::item_category(id) {
      Some(category) => groups.iter().any(|group| group.contains(&category)),
      // Ids missing from the name table follow the 其它/未知 toggle.
      None => other_allowed,
    };
    let Some(target) = self.target.as_mut() else {
      return Err("尚未打开目标存档".to_owned());
    };
    let report = edit::transfer_item_box_filtered(target.document.payload_mut(), source.document.payload(), keep)
      .map_err(|error| error.to_string())?;
    self.refresh_target_items();
    Ok(format!(
      "道具箱合并完成: {} 槽中取自来源 {} 件,保留目标 {} 件 (来源共 {} 件道具)",
      report.slots, report.transferred, report.kept, report.source_items
    ))
  }

  /// Transfer only the ticked equipment components.
  fn transfer_equipment_selected(&mut self) -> Result<String, String> {
    let Some(source) = self.source.as_ref() else {
      return Err("尚未打开来源存档".to_owned());
    };
    let Some(target) = self.target.as_mut() else {
      return Err("尚未打开目标存档".to_owned());
    };
    let parts = edit::EquipParts {
      equip_box: self.xfer_equip_box,
      equip_manager: self.xfer_equip_manager,
      item_my_set: self.xfer_item_my_set,
    };
    let report =
      edit::transfer_equipment_parts(target.document.payload_mut(), source.document.payload(), parts)
        .map_err(|error| error.to_string())?;
    self.refresh_target_items();
    let partial = parts != edit::EquipParts::all();
    Ok(format!(
      "装备转移完成: {};装备箱 {} 槽 {} 件{}",
      report
        .classes
        .iter()
        .map(|(label, fields)| format!("{label}={fields}字段"))
        .collect::<Vec<_>>()
        .join(", "),
      report.box_slots,
      report.box_used,
      if partial { " (部分转移:进游戏后请先检查穿着与装备组合)" } else { "" }
    ))
  }

  /// Re-reads the item box summary after an in-memory mutation.
  fn refresh_target_items(&mut self) {
    let read = self
      .target
      .as_ref()
      .map(|opened| edit::read_item_box(opened.document.payload()));
    let items: Vec<(u32, u32)> = match read {
      Some(Ok(entries)) => entries.into_iter().map(|entry| (entry.id, entry.num)).collect(),
      _ => return,
    };
    let len = items.len();
    if let Some(opened) = self.target.as_mut() {
      opened.box_items = items;
    }
    self.box_edits = vec![String::new(); len];
    self.dirty = true;
  }

  /// Snapshots the opened save and hands it to the worker to write as a new
  /// file next to the original.
  fn save_target_to_new_file(&mut self, ui: &egui::Ui) {
    let Some(opened) = self.target.as_ref() else {
      return;
    };
    let output = derived_output(&opened.path, "edited");
    if output == opened.path {
      self.log_line("✘ 输出路径与源文件相同;源存档永远不会被修改");
      return;
    }
    let snapshot = Box::new(OpenedSave {
      path: opened.path.clone(),
      document: opened.document.clone(),
      hunter_name: opened.hunter_name.clone(),
      box_items: opened.box_items.clone(),
    });
    self.spawn(ui.ctx(), WorkerTask::Save { document: snapshot, output });
  }

  /// Writes the opened save back to its original path, taking a fresh
  /// directory backup first so the write is always reversible.
  fn save_target_in_place(&mut self, ui: &egui::Ui) {
    let Some(opened) = self.target.as_ref() else {
      return;
    };
    let Some(backup_dir) = opened.path.parent().map(Path::to_path_buf) else {
      return;
    };
    let snapshot = Box::new(OpenedSave {
      path: opened.path.clone(),
      document: opened.document.clone(),
      hunter_name: opened.hunter_name.clone(),
      box_items: opened.box_items.clone(),
    });
    self.spawn(ui.ctx(), WorkerTask::SaveInPlace { document: snapshot, backup_dir });
  }
}

// ---------------------------------------------------------------------------
// Background tasks
// ---------------------------------------------------------------------------

enum WorkerTask {
  Open { path: PathBuf, steamid64: u64, is_source: bool },
  Save { document: Box<OpenedSave>, output: PathBuf },
  /// Writes over the original save path, after a fresh backup of its
  /// directory; the write is skipped when the backup fails.
  SaveInPlace { document: Box<OpenedSave>, backup_dir: PathBuf },
  Backup { dir: PathBuf },
  Restore { backup: PathBuf, target_dir: PathBuf },
}

impl WorkerTask {
  fn describe(&self) -> String {
    match self {
      WorkerTask::Open { path, is_source, .. } => {
        if *is_source {
          format!("打开来源存档: {}", path.display())
        } else {
          format!("打开存档: {}", path.display())
        }
      }
      WorkerTask::Save { output, .. } => format!("保存到: {}", output.display()),
      WorkerTask::SaveInPlace { backup_dir, .. } => {
        format!("写入原存档 (先备份 {})", backup_dir.display())
      }
      WorkerTask::Backup { dir } => format!("备份目录: {}", dir.display()),
      WorkerTask::Restore { backup, target_dir } => {
        format!("还原 {} 到 {}", backup.display(), target_dir.display())
      }
    }
  }

  fn run(self) -> WorkerEvent {
    match self {
      WorkerTask::Open { path, steamid64, is_source } => {
        let result =
          OpenedSave::open(path, steamid64).map(Box::new).map_err(|error| format!("{error:?}"));
        WorkerEvent::Opened(result, is_source)
      }
      WorkerTask::Save { document, output } => {
        let result =
          document.document.write_to(&output).map(|_| (output, None)).map_err(|error| format!("{error:?}"));
        WorkerEvent::Saved(result)
      }
      WorkerTask::SaveInPlace { document, backup_dir } => {
        let run = || -> anyhow::Result<(PathBuf, Option<PathBuf>)> {
          let report = crate::archive::backup_dir(&backup_dir, None)?;
          let backup = report.destination.clone();
          let output = document.path.clone();
          document.document.write_to(&output)?;
          Ok((output, Some(backup)))
        };
        let result = run().map_err(|error| format!("{error:?}"));
        WorkerEvent::Saved(result)
      }
      WorkerTask::Backup { dir } => {
        let result = crate::archive::backup_dir(&dir, None)
          .map(|report| {
            format!(
              "已备份 {} 个文件 ({} 字节) 到 {},每份拷贝已逐字节校验",
              report.files.len(),
              report.total_bytes(),
              report.destination.display()
            )
          })
          .map_err(|error| format!("{error:?}"));
        WorkerEvent::Report(result)
      }
      WorkerTask::Restore { backup, target_dir } => {
        let result = crate::archive::restore_dir(&backup, &target_dir, true)
          .map(|report| {
            format!(
              "已还原 {} 个文件 ({} 字节) 到 {},请重新打开存档查看",
              report.files.len(),
              report.total_bytes(),
              target_dir.display()
            )
          })
          .map_err(|error| format!("{error:?}"));
        WorkerEvent::Report(result)
      }
    }
  }
}

// ---------------------------------------------------------------------------
// eframe app
// ---------------------------------------------------------------------------

impl GuiApp {
  /// Diagnostics for IME debugging: with `MHR_IME_DEBUG=1`, appends every
  /// keyboard/IME input event egui receives to `ime_debug.log` next to the
  /// executable. Cheap when the variable is unset.
  fn log_input_events_if_enabled(&mut self, ui: &egui::Ui) {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    if !*ENABLED.get_or_init(|| std::env::var_os("MHR_IME_DEBUG").is_some()) {
      return;
    }
    let events = ui.input(|input| input.events.clone());
    if events.is_empty() {
      return;
    }
    let Ok(mut file) = std::fs::OpenOptions::new()
      .create(true)
      .append(true)
      .open("ime_debug.log")
    else {
      return;
    };
    use std::io::Write as _;
    let stamp = std::time::SystemTime::now()
      .duration_since(std::time::UNIX_EPOCH)
      .map(|d| d.as_millis())
      .unwrap_or(0);
    let _ = writeln!(file, "=== frame {stamp} ({} events)", events.len());
    for event in events {
      let _ = writeln!(file, "  {event:?}");
    }
  }
}

impl eframe::App for GuiApp {
  fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
    self.log_input_events_if_enabled(ui);
    if let Some(receiver) = &self.receiver {
      match receiver.try_recv() {
        Ok(WorkerEvent::Opened(result, is_source)) => {
          self.busy = false;
          self.receiver = None;
          if let Err(error) = &result {
            self.status_error = Some(error.clone());
            // A wrong SteamID64 is the common failure; the save path names the
            // 32-bit account id, so derive the correct value automatically.
            let failed_path = if is_source { &self.source_path } else { &self.target_path };
            if let Some(account) = account_id_from_path(Path::new(failed_path.trim()))
              && self.steamid64.trim() != steamid64_from_account(account).to_string()
            {
              self.steamid64 = steamid64_from_account(account).to_string();
              self.log_line(&format!(
                "⚠ 已根据存档路径把 SteamID64 自动修正为 {},请重试",
                self.steamid64
              ));
            }
          } else {
            self.status_error = None;
          }
          match result {
            Ok(opened) => {
              let (money, total) = edit::read_money(opened.document.payload()).unwrap_or((0, 0));
              let points = edit::read_points(opened.document.payload()).unwrap_or(0);
              self.money_input = money.to_string();
              self.total_input = total.to_string();
              self.points_input = points.to_string();
              let ranks = edit::read_ranks(opened.document.payload());
              self.hunter_rank_input = ranks.map(|r| r.hunter.to_string()).unwrap_or_default();
              self.master_rank_input = ranks.map(|r| r.master.to_string()).unwrap_or_default();
              self.mystery_rank_input =
                ranks.map(|r| r.mystery_research.to_string()).unwrap_or_default();
              self.box_edits = vec![String::new(); opened.box_items.len()];
              self.refresh_progress_inputs();
              self.log_line(&format!(
                "✔ 已打开{}: {} ({})",
                if is_source { "来源存档" } else { "目标存档" },
                opened.path.display(),
                opened.summary()
              ));
              if is_source {
                self.source = Some(opened);
              } else {
                self.target = Some(opened);
                self.dirty = false;
                self.reopen_clicked_once = false;
              }
            }
            Err(error) => self.log_line(&format!("✘ 打开失败: {error}")),
          }
        }
        Ok(WorkerEvent::Saved(result)) => {
          self.busy = false;
          self.receiver = None;
          match result {
            Ok((output, backup)) => {
              self.dirty = false;
              self.log_line(&format!(
                "✔ 已写入: {}{}",
                output.display(),
                backup
                  .map(|backup| format!(" (已自动备份到 {})", backup.display()))
                  .unwrap_or_default()
              ));
            }
            Err(error) => self.log_line(&format!("✘ 保存失败: {error}")),
          }
        }
        Ok(WorkerEvent::Report(result)) => {
          self.busy = false;
          self.receiver = None;
          self.restore_confirm = None;
          let restored_ok = result.is_ok();
          match result {
            Ok(text) => self.log_line(&format!("✔ {text}")),
            Err(error) => self.log_line(&format!("✘ {error}")),
          }
          // A restore rewrote the save directory under us; reload the target
          // so the editor shows what is actually on disk now.
          if restored_ok
            && let (Ok(steamid64), Some(opened)) = (self.parsed_steamid64(), self.target.as_ref())
          {
            let task =
              WorkerTask::Open { path: opened.path.clone(), steamid64, is_source: false };
            self.spawn(ui.ctx(), task);
          }
        }
        Err(TryRecvError::Empty) => {}
        Err(TryRecvError::Disconnected) => {
          self.busy = false;
          self.receiver = None;
          self.log_line("✘ 后台线程异常退出");
        }
      }
    }

    egui::CentralPanel::default().show(ui, |ui| {
      self.show_top_bar(ui);
      ui.add_space(4.0);
      self.show_tab_bar(ui);
      ui.add_space(4.0);
      // Reserve room for the save bar and log below; without an explicit cap
      // the tab content claims every remaining pixel and pushes the save
      // button out of the visible area.
      let bottom_reserve = 180.0;
      let content_height = (ui.available_height() - bottom_reserve).max(120.0);
      egui::ScrollArea::vertical().auto_shrink(false).max_height(content_height).show(ui, |ui| {
        match self.tab {
          TAB_INFO => self.show_info_tab(ui),
          TAB_PLAYER => self.show_player_tab(ui),
          TAB_ITEMBOX => self.show_itembox_tab(ui),
          TAB_EQUIP => self.show_equip_tab(ui),
          TAB_TRANSFER => self.show_transfer_tab(ui),
          TAB_PROGRESS => self.show_progress_tab(ui),
          TAB_BACKUP => self.show_backup_tab(ui),
          _ => self.show_help_tab(ui),
        }
      });

      ui.add_space(6.0);
      ui.separator();
      self.show_save_bar(ui);
      ui.label("运行日志");
      egui::ScrollArea::vertical().stick_to_bottom(true).max_height(120.0).show(ui, |ui| {
        ui.monospace(if self.log.is_empty() { "(就绪)" } else { &self.log });
      });
    });

    // Keep the window's IME context associated at all times. winit dissociates
    // it at window creation, and egui disassociates it again whenever no
    // TextEdit has focus (Windows: ImmAssociateContextEx IACE_CHILDREN); on
    // some Windows builds the IME then never composes again — keys are
    // swallowed with no preedit or candidate window until a refocus. eframe
    // runs the viewport commands after egui's own IME handling, so re-allowing
    // IME on the frames where egui turned it off wins the race. Frames where a
    // TextEdit is focused (composition in progress) are left untouched. The
    // cost is that a stray composition can start while no text field is
    // focused; harmless here because nothing else consumes plain typing.
    let ime_allowed_by_text_edit = ui.ctx().output(|output| output.ime.is_some());
    if !ime_allowed_by_text_edit {
      ui.ctx().send_viewport_cmd(egui::ViewportCommand::IMEAllowed(true));
    }

    if self.busy {
      ui.ctx().request_repaint_after(Duration::from_millis(120));
    }
  }
}

// ---------------------------------------------------------------------------
// UI sections
// ---------------------------------------------------------------------------

impl GuiApp {
  fn show_top_bar(&mut self, ui: &mut egui::Ui) {
    ui.heading("MHRise 存档编辑器 (Steam)");
    ui.add_space(4.0);
    Grid::new("top-bar").num_columns(4).spacing([8.0, 6.0]).show(ui, |ui| {
      ui.label("快速选择");
      let selected = self
        .discovered
        .iter()
        .find(|(path, _, _)| path.display().to_string() == self.target_path)
        .map(|(_, label, _)| label.clone())
        .unwrap_or_else(|| "(未发现存档)".to_owned());
      egui::ComboBox::from_id_salt("discovered-saves").selected_text(selected).show_ui(ui, |ui| {
        for (path, label, account) in &self.discovered {
          if ui.selectable_value(&mut self.target_path, path.display().to_string(), label).clicked()
            && let Some(account) = account
          {
            // The save folder name is the 32-bit account id; derive the
            // SteamID64 automatically so users never have to look it up.
            self.steamid64 = steamid64_from_account(*account).to_string();
            let note = format!(
              "已根据存档路径自动填入 SteamID64: {}
",
              self.steamid64
            );
            self.log.push_str(&note);
          }
        }
      });
      ui.label("SteamID64");
      ui.add_sized([200.0, 20.0], egui::TextEdit::singleline(&mut self.steamid64));
      ui.end_row();

      ui.label("目标存档");
      ui.add_sized([420.0, 20.0], egui::TextEdit::singleline(&mut self.target_path));
      if ui.add_enabled(!self.busy, egui::Button::new("浏览…")).clicked()
        && let Some(path) = rfd::FileDialog::new()
          .add_filter("MHRise 存档", &["bin"])
          .set_title("选择目标存档")
          .pick_file()
      {
        self.target_path = path.display().to_string();
      }
      let steamid = self.parsed_steamid64();
      let open_label = if self.dirty && !self.reopen_clicked_once {
        "打开存档 (有未保存修改)"
      } else {
        "打开存档"
      };
      if ui
        .add_enabled(
          !self.busy && steamid.is_ok() && !self.target_path.trim().is_empty(),
          egui::Button::new(open_label),
        )
        .clicked()
      {
        if self.dirty && !self.reopen_clicked_once {
          self.reopen_clicked_once = true;
          self.log_line("⚠ 存在未保存的修改;再次点击“打开存档”将放弃这些修改并重新打开");
        } else {
          let task = WorkerTask::Open {
            path: PathBuf::from(self.target_path.trim()),
            steamid64: steamid.expect("checked"),
            is_source: false,
          };
          self.reopen_clicked_once = false;
          self.spawn(ui.ctx(), task);
        }
      }
      ui.end_row();
    });
    if let Some(error) = &self.status_error {
      ui.colored_label(egui::Color32::LIGHT_RED, format!("✘ 上次操作失败: {error}"));
    }
    ui.label(match (&self.target, self.busy) {
      (_, true) => format!("⏳ {}…", self.busy_text),
      (Some(opened), _) => format!(
        "已打开: {} ({}){}",
        opened.path.display(),
        opened.summary(),
        if self.dirty { " · ● 有未保存修改" } else { "" }
      ),
      (None, _) => "尚未打开存档".to_owned(),
    });
  }

  fn show_tab_bar(&mut self, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
      for (tab, label) in [
        (TAB_INFO, "一般信息"),
        (TAB_PLAYER, "玩家数据"),
        (TAB_ITEMBOX, "道具箱编辑"),
        (TAB_EQUIP, "装备组合"),
        (TAB_TRANSFER, "存档转移"),
        (TAB_PROGRESS, "进度与解禁"),
        (TAB_BACKUP, "备份还原"),
        (TAB_HELP, "帮助"),
      ] {
        ui.selectable_value(&mut self.tab, tab, label);
      }
    });
    ui.separator();
  }

  fn show_info_tab(&mut self, ui: &mut egui::Ui) {
    let Some(opened) = self.target.as_ref() else {
      ui.label("尚未打开存档:先用顶部“快速选择”或“浏览…”选定存档,再点“打开存档”。");
      return;
    };
    Grid::new("info").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
      ui.label("猎人名称");
      ui.label(if opened.hunter_name.is_empty() {
        "(未找到)".to_owned()
      } else {
        opened.hunter_name.clone()
      });
      ui.end_row();
      ui.label("Curve Index");
      ui.label(opened.document.curve_index().to_string());
      ui.end_row();
      ui.label("解密后大小");
      ui.label(format!("{} 字节", opened.document.original_decrypted().len()));
      ui.end_row();
      ui.label("道具箱");
      ui.label(format!(
        "{} 槽,{} 件非空",
        opened.box_items.len(),
        opened.box_items.iter().filter(|(_, num)| *num > 0).count()
      ));
      ui.end_row();
      ui.label("金钱 / 点数");
      let (money, total) = edit::read_money(opened.document.payload()).unwrap_or((0, 0));
      let points = edit::read_points(opened.document.payload()).unwrap_or(0);
      ui.label(format!("{money} / {points} (累计获得 {total})"));
      ui.end_row();
      ui.label("等级 (HR/MR/怪异研究)");
      let ranks = edit::read_ranks(opened.document.payload());
      ui.label(match ranks {
        Some(ranks) => format!("{} / {} / {}", ranks.hunter, ranks.master, ranks.mystery_research),
        None => "(未找到)".to_owned(),
      });
      ui.end_row();
      ui.label("文件");
      ui.label(opened.path.display().to_string());
      ui.end_row();
    });
    if let Ok(data) = fs::read(&opened.path)
      && let Ok(header) = parse_header(&data)
    {
      ui.label(format!(
        "容器: DSSS v{}, flags=0x{:08x}, 文件 {} 字节",
        header.version,
        header.raw_flags,
        data.len()
      ));
    }
  }

  fn show_player_tab(&mut self, ui: &mut egui::Ui) {
    if self.target.is_none() {
      ui.label("尚未打开存档。");
      return;
    }
    Grid::new("player").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
      ui.label("猎人名称");
      let name = self.target.as_ref().map(|opened| opened.hunter_name.clone()).unwrap_or_default();
      ui.label(if name.is_empty() { "(未找到)".to_owned() } else { name });
      ui.end_row();
      ui.label("金钱");
      ui.add_sized([200.0, 20.0], egui::TextEdit::singleline(&mut self.money_input));
      ui.end_row();
      ui.label("累计金钱");
      ui.add_sized([200.0, 20.0], egui::TextEdit::singleline(&mut self.total_input));
      ui.end_row();
      ui.label("点数");
      ui.add_sized([200.0, 20.0], egui::TextEdit::singleline(&mut self.points_input));
      ui.end_row();
      ui.label("猎人等级 (HR)");
      ui.add_sized([200.0, 20.0], egui::TextEdit::singleline(&mut self.hunter_rank_input));
      ui.end_row();
      ui.label("大师等级 (MR)");
      ui.add_sized([200.0, 20.0], egui::TextEdit::singleline(&mut self.master_rank_input));
      ui.end_row();
      ui.label("怪异研究等级");
      ui.add_sized([200.0, 20.0], egui::TextEdit::singleline(&mut self.mystery_rank_input));
      ui.end_row();
    });
    ui.add_space(4.0);
    ui.horizontal(|ui| {
      let steamid = self.parsed_steamid64();
      let ready = !self.busy && steamid.is_ok() && self.target.is_some();
      if ui.add_enabled(ready, egui::Button::new("应用数值修改")).clicked() {
        match self.apply_player_edits() {
          Ok(()) => self.log_line("✔ 数值修改已应用 (内存中)"),
          Err(error) => self.log_line(&format!("✘ {error}")),
        }
      }
      if ui
        .add_enabled(!self.busy && self.target.is_some(), egui::Button::new("写入原存档 (先自动备份)"))
        .clicked()
      {
        self.save_target_in_place(ui);
      }
      ui.label("(留空 = 不修改该值;名称当前为只读)");
    });
    ui.colored_label(
      egui::Color32::LIGHT_YELLOW,
      "⚠ 等级受解禁机制约束:未完成对应解禁任务(集会所/大师等级/怪异调查的紧急任务链)的存档,读档时会提示获得成就,但游戏仍会把等级钳制在当前解禁上限内。解禁状态存于任务清通标志位图中,自动改写暂不支持;请先在游戏内完成解禁任务,或改用“存档转移”整体搬运高等级存档。",
    );
  }

  fn show_itembox_tab(&mut self, ui: &mut egui::Ui) {
    if self.target.is_none() {
      ui.label("尚未打开存档。");
      return;
    }
    ui.horizontal(|ui| {
      ui.label("筛选 (名称 / 十六进制 / 十进制 ID)");
      ui.add_sized([200.0, 20.0], egui::TextEdit::singleline(&mut self.box_filter));
      let changes = self.pending_item_changes();
      let apply_text = format!("应用道具修改 ({})", changes.len());
      if ui.add_enabled(!self.busy && !changes.is_empty(), egui::Button::new(apply_text)).clicked()
      {
        match self.apply_box_edits() {
          Ok(text) => self.log_line(&format!("✔ 道具修改已应用: {text} (内存中)")),
          Err(error) => self.log_line(&format!("✘ {error}")),
        }
      }
      if ui
        .add_enabled(!self.busy && self.target.is_some(), egui::Button::new("写入原存档 (先自动备份)"))
        .clicked()
      {
        self.save_target_in_place(ui);
      }
      if self.dirty {
        ui.label(egui::RichText::new("● 有未保存修改").color(egui::Color32::LIGHT_YELLOW));
      }
    });
    if self.box_edits.len() != self.box_items_len() {
      ui.label("(加载中…)");
      return;
    }
    let filter = self.box_filter.trim().to_lowercase();
    let rows = self.visible_box_rows(&filter);
    egui::ScrollArea::vertical().max_height(320.0).auto_shrink(false).show_rows(
      ui,
      20.0,
      rows.len(),
      |ui, range| {
        for row in range {
          let index = rows[row];
          self.show_item_row(ui, index);
        }
      },
    );
    ui.label(format!(
      "显示 {} / {} 项;“新数量”留空 = 不修改;填 0 = 清空该物品",
      rows.len(),
      self.box_items_len()
    ));
  }

  fn show_item_row(&mut self, ui: &mut egui::Ui, index: usize) {
    let Some((id, num)) = self.target.as_ref().map(|opened| opened.box_items[index]) else {
      return;
    };
    ui.horizontal(|ui| {
      ui.monospace(format!("{id:08x}"));
      let name = crate::items::item_display_name(id);
      ui.add_sized([220.0, 18.0], egui::Label::new(name).truncate());
      if let Some(category) = crate::items::item_category(id) {
        ui.add_sized([80.0, 18.0], egui::Label::new(egui::RichText::new(category).weak()).truncate());
      }
      ui.add_sized([80.0, 18.0], egui::Label::new(format!("当前 {num}")));
      if num == 0 {
        ui.label("(空)");
      }
      ui.add_sized([90.0, 18.0], egui::TextEdit::singleline(&mut self.box_edits[index]));
    });
  }

  fn show_equip_tab(&mut self, ui: &mut egui::Ui) {
    if self.target.is_none() {
      ui.label("先用顶部“快速选择”或“浏览…”打开目标存档。");
      return;
    }
    ui.columns(2, |columns| {
      let mut column_iter = columns.iter_mut();
      let left = column_iter.next().expect("two columns");
      let right = column_iter.next().expect("two columns");
      // Left: the target's own loadout registers.
      left.heading("目标装备组合 (224 槽)");
      let loadouts = self
        .target
        .as_ref()
        .map(|opened| edit::read_equip_loadouts(opened.document.payload()))
        .unwrap_or_default();
      let used = loadouts.iter().filter(|loadout| loadout.is_using).count();
      left.label(format!("已登记 {used} 个;组合引用的装备箱槽位在转移时可一并搬运"));
      egui::ScrollArea::vertical().max_height(360.0).auto_shrink(false).show(left, |ui| {
        for loadout in &loadouts {
          ui.horizontal(|ui| {
            ui.monospace(format!("{:3}", loadout.index));
            if loadout.is_using {
              ui.label(&loadout.name);
            } else {
              ui.label(egui::RichText::new("(空位)").weak());
            }
          });
        }
      });

      // Right: transfer controls.
      right.heading("从来源存档转移");
      let Some(source) = self.source.as_ref() else {
        right.label("先在“存档转移”页打开来源存档,才能转移组合/单件装备。");
        return;
      };
      let source_loadouts = edit::read_equip_loadouts(source.document.payload());
      let using: Vec<&edit::EquipLoadout> =
        source_loadouts.iter().filter(|loadout| loadout.is_using).collect();
      if using.is_empty() {
        right.label("来源存档没有已登记的装备组合。");
        return;
      }
      let selected = self
        .equip_source_combo
        .min(using.len().saturating_sub(1));
      let selected_text = format!("[{}] {}", using[selected].index, using[selected].name);
      right.label("来源组合:");
      egui::ComboBox::from_id_salt("equip-source-loadout")
        .selected_text(selected_text)
        .width(260.0)
        .show_ui(right, |ui| {
          for (choice, loadout) in using.iter().enumerate() {
            ui.selectable_value(
              &mut self.equip_source_combo,
              choice,
              format!("[{}] {}", loadout.index, loadout.name),
            );
          }
        });
      right.horizontal(|ui| {
        ui.label("引用装备:");
        ui.checkbox(&mut self.equip_with_pieces, "同时搬运组合引用的装备(同槽位覆盖)");
      });
      right.horizontal(|ui| {
        ui.label("目标槽位(留空 = 第一个空位):");
        ui.add_sized([80.0, 20.0], egui::TextEdit::singleline(&mut self.equip_target_slot_text));
      });
      let source_index = using[selected].index;
      if right
        .add_enabled(!self.busy, egui::Button::new("转移装备组合"))
        .clicked()
      {
        match self.apply_equip_loadout_transfer(source_index) {
          Ok(text) => self.log_line(&format!("✔ {text} (内存中)")),
          Err(error) => self.log_line(&format!("✘ {error}")),
        }
      }

      right.add_space(10.0);
      right.separator();
      right.heading("单件装备转移");
      right.horizontal(|ui| {
        ui.label("来源槽位:");
        ui.add_sized([70.0, 20.0], egui::TextEdit::singleline(&mut self.equip_piece_source_text));
        ui.label("目标槽位:");
        ui.add_sized([70.0, 20.0], egui::TextEdit::singleline(&mut self.equip_piece_target_text));
      });
      if right
        .add_enabled(!self.busy, egui::Button::new("转移单件装备 (目标槽覆盖)"))
        .clicked()
      {
        match self.apply_equip_piece_transfer() {
          Ok(text) => self.log_line(&format!("✔ {text} (内存中)")),
          Err(error) => self.log_line(&format!("✘ {error}")),
        }
      }
      right.label("装备以装备箱槽位号定位;覆盖目标槽位前请确认其内容不再需要。");
    });
    ui.label("所有转移在内存中立即生效;确认无误后用底部按钮写盘(写入原存档会先自动备份)。");
  }

  /// Applies the selected source loadout to the chosen target slot.
  fn apply_equip_loadout_transfer(&mut self, source_index: usize) -> Result<String, String> {
    let target_slot_text = self.equip_target_slot_text.trim().to_owned();
    let target_index = if target_slot_text.is_empty() {
      None
    } else {
      Some(target_slot_text.parse::<usize>().map_err(|_| "目标槽位必须是数字")?)
    };
    {
      let Some(source) = self.source.as_ref() else {
        return Err("尚未打开来源存档".to_owned());
      };
      let Some(target) = self.target.as_mut() else {
        return Err("尚未打开目标存档".to_owned());
      };
      let slot = edit::copy_equip_loadout(
        target.document.payload_mut(),
        source.document.payload(),
        source_index,
        target_index,
        self.equip_with_pieces,
      )
      .map_err(|error| error.to_string())?;
      self.dirty = true;
      let pieces = if self.equip_with_pieces { ",引用装备已同槽位搬运" } else { "" };
      Ok(format!("装备组合已转移到槽位 {slot}{pieces}"))
    }
  }

  /// Applies the single-piece box transfer.
  fn apply_equip_piece_transfer(&mut self) -> Result<String, String> {
    let source_index = self
      .equip_piece_source_text
      .trim()
      .parse::<usize>()
      .map_err(|_| "来源槽位必须是数字")?;
    let target_index = self
      .equip_piece_target_text
      .trim()
      .parse::<usize>()
      .map_err(|_| "目标槽位必须是数字")?;
    {
      let Some(source) = self.source.as_ref() else {
        return Err("尚未打开来源存档".to_owned());
      };
      let Some(target) = self.target.as_mut() else {
        return Err("尚未打开目标存档".to_owned());
      };
      edit::copy_equip_piece(target.document.payload_mut(), source.document.payload(), source_index, target_index)
        .map_err(|error| error.to_string())?;
      self.dirty = true;
      Ok(format!("装备已从来源槽位 {source_index} 转移到目标槽位 {target_index}"))
    }
  }

  fn show_transfer_tab(&mut self, ui: &mut egui::Ui) {
    if self.target.is_none() {
      ui.label("先用顶部“快速选择”或“浏览…”选定目标存档并点“打开存档”,再进行转移。");
      return;
    }
    ui.label("把“来源存档”的数据合并进“目标存档”(同一 Steam 账号):");
    ui.add_space(4.0);
    ui.horizontal(|ui| {
      ui.label("来源快速选择");
      let selected = self
        .discovered
        .iter()
        .find(|(path, _, _)| path.display().to_string() == self.source_path)
        .map(|(_, label, _)| label.clone())
        .unwrap_or_else(|| "(未选择)".to_owned());
      egui::ComboBox::from_id_salt("discovered-sources").selected_text(selected).show_ui(ui, |ui| {
        for (path, label, account) in &self.discovered {
          if ui.selectable_value(&mut self.source_path, path.display().to_string(), label).clicked()
            && let Some(account) = account
          {
            // The save folder name is the 32-bit account id; derive the
            // SteamID64 automatically so users never have to look it up.
            self.steamid64 = steamid64_from_account(*account).to_string();
            let note = format!("已根据存档路径自动填入 SteamID64: {}\n", self.steamid64);
            self.log.push_str(&note);
          }
        }
      });
      ui.label("(与目标相同则无需转移)");
    });
    Grid::new("transfer").num_columns(3).spacing([8.0, 6.0]).show(ui, |ui| {
      ui.label("来源存档");
      ui.add_sized([420.0, 20.0], egui::TextEdit::singleline(&mut self.source_path));
      if ui.add_enabled(!self.busy, egui::Button::new("浏览…")).clicked()
        && let Some(path) = rfd::FileDialog::new()
          .add_filter("MHRise 存档", &["bin"])
          .set_title("选择来源存档")
          .pick_file()
      {
        self.source_path = path.display().to_string();
      }
      ui.end_row();
    });
    ui.horizontal(|ui| {
      let steamid = self.parsed_steamid64();
      let open_ready = !self.busy
        && steamid.is_ok()
        && !self.source_path.trim().is_empty()
        && self.source.is_none();
      if ui.add_enabled(open_ready, egui::Button::new("打开来源存档")).clicked() {
        let task = WorkerTask::Open {
          path: PathBuf::from(self.source_path.trim()),
          steamid64: steamid.expect("checked"),
          is_source: true,
        };
        self.spawn(ui.ctx(), task);
      }
      if self.source.is_some()
        && ui.add_enabled(!self.busy, egui::Button::new("卸载来源存档")).clicked()
      {
        self.source = None;
        self.log_line("已卸载来源存档");
      }
      if let Some(source) = &self.source {
        ui.label(format!("来源: {} ({})", source.path.display(), source.summary()));
      }
    });

    ui.add_space(6.0);
    ui.separator();
    ui.heading("道具箱转移");
    ui.horizontal(|ui| {
      ui.label("取自来源的类别:");
      for ((label, _), on) in ITEM_CATEGORY_GROUPS.iter().zip(&mut self.xfer_cats) {
        ui.checkbox(on, *label);
      }
      if ui.small_button("全选").clicked() {
        self.xfer_cats.iter_mut().for_each(|on| *on = true);
      }
      if ui.small_button("清空").clicked() {
        self.xfer_cats.iter_mut().for_each(|on| *on = false);
      }
    });
    ui.horizontal(|ui| {
      let both_ready = !self.busy && self.source.is_some();
      let any_cat = self.xfer_cats.iter().any(|on| *on);
      if ui
        .add_enabled(both_ready && any_cat, egui::Button::new("转移道具箱 (按类别合并)"))
        .clicked()
      {
        match self.transfer_items_filtered() {
          Ok(text) => self.log_line(&format!("✔ {text} (内存中)")),
          Err(error) => self.log_line(&format!("✘ {error}")),
        }
      }
      ui.label("仅勾选类别的槽位取自来源,其余保留目标原样;全部勾选 = 完整替换");
    });

    ui.add_space(6.0);
    ui.separator();
    ui.heading("进度转移");
    ui.horizontal(|ui| {
      ui.checkbox(&mut self.xfer_progress, "等级进度 (HR/MR/怪异研究等级 + 点数)");
    });
    ui.horizontal(|ui| {
      let both_ready = !self.busy && self.source.is_some() && self.xfer_progress;
      if ui.add_enabled(both_ready, egui::Button::new("转移等级进度")).clicked() {
        match self.transfer_progress() {
          Ok(text) => self.log_line(&format!("✔ {text} (内存中)")),
          Err(error) => self.log_line(&format!("✘ {error}")),
        }
      }
      ui.label("⚠ 新档未完成解禁任务时,转移后的等级仍会被游戏钳制在当前上限内");
    });

    ui.add_space(6.0);
    ui.separator();
    ui.heading("装备转移");
    ui.horizontal(|ui| {
      ui.checkbox(&mut self.xfer_equip_box, "装备箱 (武器/防具/护石)");
      ui.checkbox(&mut self.xfer_equip_manager, "穿着与装备组合登记");
      ui.checkbox(&mut self.xfer_item_my_set, "道具袋组合");
    });
    ui.horizontal(|ui| {
      let both_ready = !self.busy && self.source.is_some();
      let any_part = self.xfer_equip_box || self.xfer_equip_manager || self.xfer_item_my_set;
      if ui
        .add_enabled(both_ready && any_part, egui::Button::new("转移装备 (按所选组件)"))
        .clicked()
      {
        match self.transfer_equipment_selected() {
          Ok(text) => self.log_line(&format!("✔ {text} (内存中)")),
          Err(error) => self.log_line(&format!("✘ {error}")),
        }
      }
    });
    if !(self.xfer_equip_box && self.xfer_equip_manager && self.xfer_item_my_set) {
      ui.colored_label(
        egui::Color32::LIGHT_YELLOW,
        "⚠ 部分转移时,装备组合/穿着引用的装备箱序号可能对应不同装备,进游戏后请先检查",
      );
    }
    ui.label("转移在内存中立即生效;确认无误后用下方“保存为新文件”写盘。");
  }

  fn show_progress_tab(&mut self, ui: &mut egui::Ui) {
    if self.target.is_none() {
      ui.label("先用顶部“快速选择”或“浏览…”打开一个存档。");
      return;
    }
    ui.heading("等级与累计点数");
    ui.label("点数是升级时的累计进度,等级钳制解除后游戏按点数推进等级;留空 = 不修改。");
    ui.add_space(4.0);
    Grid::new("progress-points").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
      let row = |ui: &mut egui::Ui, label: &str, text: &mut String| {
        ui.label(label);
        ui.add_sized([200.0, 20.0], egui::TextEdit::singleline(text));
        ui.end_row();
      };
      row(ui, "猎人等级点数 (HR Point)", &mut self.hr_point_input);
      row(ui, "大师等级点数 (MR Point)", &mut self.mr_point_input);
      row(ui, "怪异研究点数", &mut self.mystery_point_input);
    });
    ui.horizontal(|ui| {
      if ui.add_enabled(!self.busy, egui::Button::new("应用点数修改")).clicked() {
        match self.apply_point_edits() {
          Ok(text) => self.log_line(&format!("✔ {text} (内存中)")),
          Err(error) => self.log_line(&format!("✘ {error}")),
        }
      }
      ui.label("(点数仅是进度累积,不改变解禁钳制;1 ≤ 值 ≤ 99999999)");
    });

    ui.add_space(8.0);
    ui.separator();
    ui.heading("名片解禁标志 (显示副本)");
    ui.horizontal_wrapped(|ui| {
      for ((_, label), on) in edit::GUILD_FLAGS.iter().zip(&mut self.guild_flags) {
        ui.checkbox(on, *label);
      }
    });
    ui.horizontal(|ui| {
      if ui.add_enabled(!self.busy, egui::Button::new("应用名片标志")).clicked() {
        let values: Vec<(u32, bool)> = edit::GUILD_FLAGS
          .iter()
          .zip(&self.guild_flags)
          .map(|((hash, _), on)| (*hash, *on))
          .collect();
        match self.apply_guild_flags(&values) {
          Ok(text) => self.log_line(&format!("✔ {text} (内存中)")),
          Err(error) => self.log_line(&format!("✘ {error}")),
        }
      }
      ui.label("这些是猎人名片上的显示副本;实测解禁钳制不完全由它们决定");
    });

    ui.add_space(8.0);
    ui.separator();
    ui.heading("为什么改了等级却不生效?");
    ui.label("· 游戏对等级有解禁钳制:未完成对应解禁任务链时,读档会提示获得成就,但显示等级仍被压回当前上限");
    ui.label("· 解禁状态记录在任务清通标志位图中(打包结构,任务到位的映射需要游戏数据表),本工具暂不自动改写");
    ui.label("· 想让新档达到高等级:在游戏内完成解禁任务,或用“存档转移”把高等级档的装备/道具/等级进度搬过来(解禁仍受任务进度约束)");
  }

  /// Applies the three point-accumulator edits from the 进度与解禁 tab.
  fn apply_point_edits(&mut self) -> Result<String, String> {
    let parse = |text: &str, name: &str| -> Result<Option<u32>, String> {
      let trimmed = text.trim();
      if trimmed.is_empty() {
        return Ok(None);
      }
      let value = trimmed.parse::<u32>().map_err(|_| format!("{name} 必须是数字: {text:?}"))?;
      if value == 0 || value > 99_999_999 {
        return Err(format!("{name} 超出范围 (1-99999999): {value}"));
      }
      Ok(Some(value))
    };
    let hr = parse(&self.hr_point_input, "猎人等级点数")?;
    let mr = parse(&self.mr_point_input, "大师等级点数")?;
    let mystery = parse(&self.mystery_point_input, "怪异研究点数")?;
    let Some(opened) = self.target.as_mut() else {
      return Err("尚未打开目标存档".to_owned());
    };
    let mut applied = 0usize;
    for (value, hash, label) in [
      (hr, edit::HUNTER_RANK_POINT, "_HunterRankPoint"),
      (mr, edit::MASTER_RANK_POINT, "_MasterRankPoint"),
      (mystery, edit::MYSTERY_RESEARCH_POINT, "_MysteryResearchPoint"),
    ] {
      if let Some(value) = value
        && value != edit::read_scalar(opened.document.payload(), edit::PROGRESS_SAVE_CLASS, hash)
          .unwrap_or(0)
      {
        edit::set_scalar(opened.document.payload_mut(), edit::PROGRESS_SAVE_CLASS, hash, value, label)
          .map_err(|error| error.to_string())?;
        applied += 1;
      }
    }
    if applied == 0 {
      return Err("没有可应用的修改".to_owned());
    }
    self.dirty = true;
    Ok(format!("更新了 {applied} 项点数"))
  }

  /// Copies ProgressSaveData (ranks + point accumulators) from the source.
  fn transfer_progress(&mut self) -> Result<String, String> {
    {
      let Some(source) = self.source.as_ref() else {
        return Err("尚未打开来源存档".to_owned());
      };
      let Some(target) = self.target.as_mut() else {
        return Err("尚未打开目标存档".to_owned());
      };
      edit::copy_class_fields(
        target.document.payload_mut(),
        source.document.payload(),
        edit::PROGRESS_SAVE_CLASS,
      )
      .map_err(|error| error.to_string())?;
    }
    self.refresh_target_items();
    self.refresh_progress_inputs();
    let ranks = self
      .target
      .as_ref()
      .and_then(|target| edit::read_ranks(target.document.payload()));
    Ok(format!(
      "等级进度转移完成;目标现为 HR {} / MR {} / 怪异研究 {}",
      ranks.map(|r| r.hunter).unwrap_or(0),
      ranks.map(|r| r.master).unwrap_or(0),
      ranks.map(|r| r.mystery_research).unwrap_or(0)
    ))
  }

  /// Applies guild card unlock flags from the 进度与解禁 tab.
  fn apply_guild_flags(&mut self, values: &[(u32, bool)]) -> Result<String, String> {
    let Some(opened) = self.target.as_mut() else {
      return Err("尚未打开目标存档".to_owned());
    };
    let updated = edit::set_guild_flags(opened.document.payload_mut(), values)
      .map_err(|error| error.to_string())?;
    self.dirty = true;
    Ok(format!("更新了 {updated} 个名片标志"))
  }

  fn show_backup_tab(&mut self, ui: &mut egui::Ui) {
    let Some(target_dir) = self.target.as_ref().map(|opened| opened.path.parent().map(Path::to_path_buf)).flatten() else {
      ui.label("先用顶部“快速选择”或“浏览…”打开一个存档,才能定位其存档目录。");
      return;
    };
    ui.label(format!("当前存档目录: {}", target_dir.display()));
    ui.label(
      "备份把整个存档目录原样复制到 文档\\MHR-Save-Backups\\<账号>\\<时间戳>,逐字节校验,且位于 Steam 云同步范围之外;",
    );
    ui.label("还原则把所选备份覆盖回存档目录——覆盖前请关闭游戏,并且不要与 Steam 云同步同时进行。");
    ui.add_space(6.0);
    ui.horizontal(|ui| {
      if ui.add_enabled(!self.busy, egui::Button::new("备份整个存档目录")).clicked() {
        self.spawn(ui.ctx(), WorkerTask::Backup { dir: target_dir.clone() });
      }
      ui.label(format!(
        "已有备份 {} 份",
        backups_for_target(&target_dir).map(|list| list.len()).unwrap_or(0)
      ));
    });

    ui.add_space(6.0);
    ui.separator();
    ui.heading("已有备份 (新在前)");
    let backups = backups_for_target(&target_dir).unwrap_or_default();
    if backups.is_empty() {
      ui.label("(尚无备份;强烈建议在第一次编辑前备份一次)");
      return;
    }
    egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
      for backup in &backups {
        ui.horizontal(|ui| {
          ui.label(backup.file_name().and_then(|name| name.to_str()).unwrap_or("?").to_owned());
          let armed = self.restore_confirm.as_deref() == Some(backup.as_path());
          let label = if armed { "⚠ 再次点击确认还原" } else { "还原此备份" };
          if ui.add_enabled(!self.busy, egui::Button::new(label)).clicked() {
            if armed {
              self.restore_confirm = None;
              self.log_line(&format!("▶ 正在还原 {} …", backup.display()));
              self.spawn(ui.ctx(), WorkerTask::Restore {
                backup: backup.clone(),
                target_dir: target_dir.clone(),
              });
            } else {
              self.restore_confirm = Some(backup.clone());
              self.log_line("⚠ 还原会覆盖存档目录中的全部文件;再点一次“还原此备份”确认");
            }
          }
          if armed && ui.small_button("取消").clicked() {
            self.restore_confirm = None;
          }
        });
      }
    });
  }

  fn show_help_tab(&mut self, ui: &mut egui::Ui) {
    ui.label(r"存档目录: <Steam库>\userdata\<32位账号ID>\1446780\remote\win64_save");
    ui.label(r"· Steam 默认装在 C:\Program Files (x86)\Steam;自定义库在其它盘(例如 D:\Steam)。启动时已自动扫描所有 Steam 库。");
    ui.label("· 角色存档: data001Slot.bin ~ data003Slot.bin(游戏内最多 3 个角色槽,即读档界面的 3 个存档位)");
    ui.label("· 系统存档: data00-1.bin(全局数据,一般不需要修改);SS1_* / SS4_* / SS7_* 是相册数据");
    ui.label(
      "· 推荐流程: 备份 → 编辑 → 写入原存档(自动先备份整个目录)→ 进游戏确认;“保存为新文件”则输出 <原名>.edited.bin,需手动替换原文件",
    );
    ui.label("· Steam 云同步: 编辑时建议让 Steam 离线,避免云端旧档覆盖");
    ui.label("· 角色存档必须由游戏创建过(空槽没有引导存档时,游戏不会识别放入的文件)");
    ui.add_space(8.0);
    ui.heading("中文无法输入/没有候选词?");
    ui.label("· 本程序基于 winit 窗口库,与 TSF 模式的输入法存在已知兼容性问题(打字被吞、无候选词)");
    ui.label("· 微软拼音: 设置 → 时间和语言 → 语言和区域 → 中文 → 微软拼音 → 常规 → 兼容性 → 打开「使用以前版本的 Microsoft 输入法」");
    ui.label("· 搜狗等第三方输入法: 若同样无效,属于同一兼容性问题,可临时切换到已开启兼容模式的微软拼音;此问题影响所有 winit 应用,与本工具的存档逻辑无关");
  }

  fn show_save_bar(&mut self, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
      let ready = !self.busy && self.target.is_some();
      if ui.add_enabled(ready, egui::Button::new("写入原存档 (先自动备份)")).clicked() {
        self.save_target_in_place(ui);
      }
      if ui.add_enabled(ready, egui::Button::new("保存为新文件")).clicked() {
        self.save_target_to_new_file(ui);
      }
      if self.dirty {
        ui.label("● 有未保存修改 (内存中)");
      }
      ui.label("关闭游戏后再写入;“写入原存档”会先把整个存档目录备份到 文档\\MHR-Save-Backups");
    });
  }

  fn box_items_len(&self) -> usize {
    self.target.as_ref().map(|opened| opened.box_items.len()).unwrap_or(0)
  }

  fn visible_box_rows(&self, filter: &str) -> Vec<usize> {
    let Some(opened) = self.target.as_ref() else {
      return Vec::new();
    };
    (0..opened.box_items.len())
      .filter(|&index| {
        if filter.is_empty() {
          return true;
        }
        let (id, _) = opened.box_items[index];
        let category = crate::items::item_category(id).unwrap_or("");
        crate::items::item_name_cn(id).unwrap_or("").contains(filter)
          || category.to_lowercase().contains(filter)
          || crate::items::item_name(id).unwrap_or("").to_lowercase().contains(filter)
          || format!("{id:08x}").contains(filter)
          || format!("{id}").contains(filter)
      })
      .collect()
  }
}

// ---------------------------------------------------------------------------
// Steam save discovery
// ---------------------------------------------------------------------------

/// The backup folders recorded for the target's account, newest first.
/// Mirrors the layout `archive::documents_backup_destination` writes:
/// `Documents\MHR-Save-Backups\<account>\<timestamp>`.
fn backups_for_target(target_save: &Path) -> Option<Vec<PathBuf>> {
  let account = target_save.parent()?.parent()?.parent()?.file_name()?.to_str()?;
  let home = std::env::var_os("USERPROFILE")?;
  let root = PathBuf::from(home).join("Documents").join("MHR-Save-Backups").join(account);
  let mut backups: Vec<PathBuf> = fs::read_dir(&root)
    .ok()?
    .filter_map(|entry| entry.ok())
    .map(|entry| entry.path())
    .filter(|path| path.is_dir())
    .collect();
  backups.sort();
  backups.reverse();
  Some(backups)
}

fn path_key(path: &Path) -> String {
  let text = path.to_string_lossy().replace('/', "\\");
  text.trim_end_matches('\\').to_lowercase()
}

/// Scans every known Steam library for MHR save directories
/// (`userdata/<account>/1446780/remote/win64_save`).
fn discover_save_dirs() -> Vec<PathBuf> {
  let mut roots: Vec<PathBuf> = Vec::new();
  if let Ok(output) = std::process::Command::new("reg")
    .args(["query", r"HKCU\Software\Valve\Steam", "/v", "SteamPath"])
    .output()
  {
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
      if let Some(position) = line.find("REG_SZ") {
        let value = line[position + "REG_SZ".len()..].trim();
        if !value.is_empty() {
          roots.push(PathBuf::from(value));
        }
      }
    }
  }
  // Heuristic fallbacks for common install locations; the registry and the
  // library manifest above are the authoritative sources.
  roots.push(PathBuf::from(r"C:\Program Files (x86)\Steam"));
  roots.push(PathBuf::from(r"D:\Steam"));
  roots.push(PathBuf::from(r"E:\Steam"));

  let mut libraries = roots.clone();
  for root in &roots {
    let vdf = root.join("steamapps").join("libraryfolders.vdf");
    let Ok(text) = fs::read_to_string(&vdf) else {
      continue;
    };
    for line in text.lines() {
      let trimmed = line.trim();
      if !trimmed.starts_with("\"path\"") {
        continue;
      }
      let parts: Vec<&str> = trimmed.split('"').collect();
      if parts.len() >= 4 {
        let raw = parts[parts.len() - 2];
        if !raw.is_empty() {
          libraries.push(PathBuf::from(raw.replace("\\\\", "\\")));
        }
      }
    }
  }
  // The registry stores `d:/steam` while the manifest stores `D:\Steam`;
  // deduplicate by normalized key or the same library gets scanned twice.
  libraries.sort_by_key(|path| path_key(path));
  libraries.dedup_by(|first, second| path_key(first) == path_key(second));

  let mut saves = Vec::new();
  for library in &libraries {
    let userdata = library.join("userdata");
    let Ok(accounts) = fs::read_dir(&userdata) else {
      continue;
    };
    for account in accounts.flatten() {
      let dir = account.path().join("1446780").join("remote").join("win64_save");
      if dir.is_dir() {
        saves.push(dir);
      }
    }
  }
  saves.sort_by_key(|path| path_key(path));
  saves.dedup_by(|first, second| path_key(first) == path_key(second));
  saves
}

/// Lists the character slot files inside every discovered save directory.
fn discover_steam_slot_files() -> Vec<(PathBuf, String, Option<u32>)> {
  let mut files = Vec::new();
  let mut seen = std::collections::BTreeSet::new();
  for dir in discover_save_dirs() {
    for name in ["data001Slot.bin", "data002Slot.bin", "data003Slot.bin"] {
      let path = dir.join(name);
      let key = path_key(&path);
      if path.is_file() && seen.insert(key) {
        let account = dir
          .parent()
          .and_then(|remote| remote.parent())
          .and_then(|appid| appid.parent())
          .and_then(|account| account.file_name())
          .map(|name| name.to_string_lossy().to_string())
          .unwrap_or_default();
        files.push((path, format!("账号 {account} / {name}"), account.parse::<u32>().ok()));
      }
    }
  }
  files
}

fn derived_output(source: &Path, suffix: &str) -> PathBuf {
  let mut name = source.file_stem().and_then(|stem| stem.to_str()).unwrap_or("save").to_owned();
  name.push('.');
  name.push_str(suffix);
  name.push_str(".bin");
  source.with_file_name(name)
}

/// `userdata\<account>\1446780\...`: the component right after `userdata`
/// (one step before it in the ancestor walk) is the 32-bit account id.
fn account_id_from_path(path: &Path) -> Option<u32> {
  let ancestors: Vec<&Path> = path.ancestors().collect();
  for (index, ancestor) in ancestors.iter().enumerate() {
    if index > 0
      && let Some(name) = ancestor.file_name()
      && name.to_str()?.eq_ignore_ascii_case("userdata")
    {
      return ancestors[index - 1].file_name()?.to_str()?.parse::<u32>().ok();
    }
  }
  None
}

/// SteamID64 = 76561197960265728 + 32-bit account id.
fn steamid64_from_account(account: u32) -> u64 {
  76_561_197_960_265_728 + u64::from(account)
}
