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
const TAB_TRANSFER: usize = 3;
const TAB_HELP: usize = 4;

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
  Saved(Result<PathBuf, String>),
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
  box_filter: String,
  box_edits: Vec<String>,
  busy: bool,
  busy_text: String,
  log: String,
  receiver: Option<Receiver<WorkerEvent>>,
  tab: usize,
}

impl GuiApp {
  pub fn new(creation_context: &eframe::CreationContext) -> Self {
    let font_status = install_cjk_font(&creation_context.egui_ctx);
    let mut app = Self::default();
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

  fn transfer(&mut self, equipment: bool) -> Result<String, String> {
    let Some(source) = self.source.as_ref() else {
      return Err("尚未打开来源存档".to_owned());
    };
    let Some(target) = self.target.as_mut() else {
      return Err("尚未打开目标存档".to_owned());
    };
    let message = if equipment {
      let report =
        edit::transfer_equipment(target.document.payload_mut(), source.document.payload())
          .map_err(|error| error.to_string())?;
      format!(
        "装备转移完成: {};装备箱 {} 槽 {} 件",
        report
          .classes
          .iter()
          .map(|(label, fields)| format!("{label}={fields}字段"))
          .collect::<Vec<_>>()
          .join(", "),
        report.box_slots,
        report.box_used
      )
    } else {
      let report =
        edit::transfer_item_box(target.document.payload_mut(), source.document.payload())
          .map_err(|error| error.to_string())?;
      format!("道具箱转移完成: {} 槽,{} 件道具", report.slots, report.items)
    };
    target.box_items = edit::read_item_box(target.document.payload())
      .map_err(|error| error.to_string())?
      .into_iter()
      .map(|entry| (entry.id, entry.num))
      .collect();
    self.box_edits = vec![String::new(); target.box_items.len()];
    self.dirty = true;
    Ok(message)
  }
}

// ---------------------------------------------------------------------------
// Background tasks
// ---------------------------------------------------------------------------

enum WorkerTask {
  Open { path: PathBuf, steamid64: u64, is_source: bool },
  Save { document: Box<OpenedSave>, output: PathBuf },
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
          document.document.write_to(&output).map(|_| output).map_err(|error| format!("{error:?}"));
        WorkerEvent::Saved(result)
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
              self.box_edits = vec![String::new(); opened.box_items.len()];
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
            Ok(output) => {
              self.dirty = false;
              self.log_line(&format!("✔ 已保存: {}", output.display()));
            }
            Err(error) => self.log_line(&format!("✘ 保存失败: {error}")),
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
      egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| match self.tab {
        TAB_INFO => self.show_info_tab(ui),
        TAB_PLAYER => self.show_player_tab(ui),
        TAB_ITEMBOX => self.show_itembox_tab(ui),
        TAB_TRANSFER => self.show_transfer_tab(ui),
        _ => self.show_help_tab(ui),
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
        (TAB_TRANSFER, "存档转移"),
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
    });
    ui.add_space(4.0);
    ui.horizontal(|ui| {
      let steamid = self.parsed_steamid64();
      let ready = !self.busy && steamid.is_ok() && self.target.is_some();
      if ui.add_enabled(ready, egui::Button::new("应用数值修改")).clicked() {
        match self.apply_player_edits() {
          Ok(()) => self.log_line("✔ 数值修改已应用 (内存中,记得“保存为新文件”)"),
          Err(error) => self.log_line(&format!("✘ {error}")),
        }
      }
      ui.label("(留空 = 不修改该值;名称当前为只读)");
    });
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

  fn show_transfer_tab(&mut self, ui: &mut egui::Ui) {
    ui.label("把“来源存档”的道具/装备合并进“目标存档”(同一 Steam 账号,先各自打开):");
    ui.add_space(4.0);
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
        && self.target.is_some()
        && self.source.is_none();
      if ui.add_enabled(open_ready, egui::Button::new("打开来源存档")).clicked() {
        let task = WorkerTask::Open {
          path: PathBuf::from(self.source_path.trim()),
          steamid64: steamid.expect("checked"),
          is_source: true,
        };
        self.spawn(ui.ctx(), task);
      }
      let both_ready = !self.busy && self.target.is_some() && self.source.is_some();
      if ui.add_enabled(both_ready, egui::Button::new("转移道具箱")).clicked() {
        match self.transfer(false) {
          Ok(text) => self.log_line(&format!("✔ {text} (内存中)")),
          Err(error) => self.log_line(&format!("✘ {error}")),
        }
      }
      if ui.add_enabled(both_ready, egui::Button::new("转移装备+护石+组合")).clicked() {
        match self.transfer(true) {
          Ok(text) => self.log_line(&format!("✔ {text} (内存中)")),
          Err(error) => self.log_line(&format!("✘ {error}")),
        }
      }
      let source_note = self
        .source
        .as_ref()
        .map(|source| format!("来源: {} ({})", source.path.display(), source.summary()));
      if self.source.is_some()
        && ui.add_enabled(!self.busy, egui::Button::new("卸载来源存档")).clicked()
      {
        self.source = None;
        self.log_line("已卸载来源存档");
      }
      if let Some(note) = source_note {
        ui.label(note);
      }
    });
    ui.label("转移在内存中立即生效;确认无误后用底部“保存为新文件”写盘。");
  }

  fn show_help_tab(&mut self, ui: &mut egui::Ui) {
    ui.label(r"存档目录: <Steam库>\userdata\<32位账号ID>\1446780\remote\win64_save");
    ui.label(r"· Steam 默认装在 C:\Program Files (x86)\Steam;自定义库在其它盘(例如 D:\Steam)。启动时已自动扫描所有 Steam 库。");
    ui.label("· 角色存档: data001Slot.bin ~ data003Slot.bin(游戏内最多 3 个角色槽,即读档界面的 3 个存档位)");
    ui.label("· 系统存档: data00-1.bin(全局数据,一般不需要修改);SS1_* / SS4_* / SS7_* 是相册数据");
    ui.label(
      "· 推荐流程: 备份 → 编辑 → 保存为新文件 → 关闭游戏 → 用输出文件替换原存档 → 进游戏确认",
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
      if ui.add_enabled(ready, egui::Button::new("保存为新文件")).clicked()
        && let Some(opened) = self.target.as_ref()
      {
        let output = derived_output(&opened.path, "edited");
        if output == opened.path {
          self.log_line("✘ 输出路径与源文件相同;源存档永远不会被修改");
        } else {
          let document = Box::new(OpenedSave {
            path: opened.path.clone(),
            document: opened.document.clone(),
            hunter_name: opened.hunter_name.clone(),
            box_items: opened.box_items.clone(),
          });
          self.spawn(ui.ctx(), WorkerTask::Save { document, output });
        }
      }
      if self.dirty {
        ui.label("● 有未保存修改 (内存中)");
      }
      ui.label("输出文件: <原名>.edited.bin,替换原存档前请关闭游戏");
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
