//! eframe/egui front end for the save editor.
//!
//! Every operation runs on a background thread and writes its output to a NEW
//! file next to the selected save; the selected save itself is never
//! modified. The user swaps the output file in with the game closed.

use std::{
  fmt::Write as _,
  fs,
  path::{Path, PathBuf},
  sync::mpsc::{self, Receiver, TryRecvError},
  thread,
  time::Duration,
};

use eframe::egui;
use egui::Grid;

use crate::{container::SteamSave, discover::discover_save_files, edit, format::parse_header};

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

#[derive(Debug)]
enum Operation {
  Inspect { target: PathBuf, steamid64: u64 },
  SetMoney { target: PathBuf, steamid64: u64, value: u32, total_added: Option<u32> },
  SetPoints { target: PathBuf, steamid64: u64, value: u32 },
  TransferItems { target: PathBuf, source: PathBuf, steamid64: u64 },
  TransferEquipment { target: PathBuf, source: PathBuf, steamid64: u64 },
  LoadBox { target: PathBuf, steamid64: u64 },
  SetItems { target: PathBuf, steamid64: u64, changes: Vec<(u32, u32)> },
}

impl Operation {
  fn describe(&self) -> String {
    match self {
      Operation::Inspect { .. } => "读取存档信息".to_owned(),
      Operation::SetMoney { value, .. } => format!("设置金钱为 {value}"),
      Operation::SetPoints { value, .. } => format!("设置点数为 {value}"),
      Operation::TransferItems { .. } => "转移道具箱".to_owned(),
      Operation::TransferEquipment { .. } => "转移装备(装备箱+护石+组合)".to_owned(),
      Operation::LoadBox { .. } => "读取道具箱".to_owned(),
      Operation::SetItems { changes, .. } => format!("修改 {} 项道具数量", changes.len()),
    }
  }

  fn run(self) -> anyhow::Result<OperationOutcome> {
    match self {
      Operation::Inspect { target, steamid64 } => {
        let document = SteamSave::open_path(&target, steamid64)?;
        let files = discover_save_files(&target);
        let file_note = match files {
          Ok(list) => format!("(发现 {} 个存档文件)", list.len()),
          Err(_) => String::new(),
        };
        let mut text = format!(
          "读取成功: Curve Index {}, 解密后 {} 字节 {file_note}\n",
          document.curve_index(),
          document.original_decrypted().len()
        );
        if let Ok(data) = fs::read(&target)
          && let Ok(header) = parse_header(&data)
        {
          let _ = writeln!(
            text,
            "DSSS v{}, flags=0x{:08x}, 文件 {} 字节",
            header.version,
            header.raw_flags,
            data.len()
          );
        }
        Ok((text, None))
      }
      Operation::SetMoney { target, steamid64, value, total_added } => {
        let mut document = SteamSave::open_path(&target, steamid64)?;
        let report = edit::set_money(document.payload_mut(), value, total_added)?;
        let output = write_edited(&mut document, &target, "edited")?;
        Ok((
          format!(
            "金钱已设置: {} -> {} (更新 {} 处)\n输出: {}",
            report.before.first().copied().unwrap_or(0),
            value,
            report.updated,
            output.display()
          ),
          None,
        ))
      }
      Operation::SetPoints { target, steamid64, value } => {
        let mut document = SteamSave::open_path(&target, steamid64)?;
        let report = edit::set_village_points(document.payload_mut(), value)?;
        let output = write_edited(&mut document, &target, "edited")?;
        Ok((
          format!(
            "点数已设置: {} -> {} (更新 {} 处)\n输出: {}",
            report.before.first().copied().unwrap_or(0),
            value,
            report.updated,
            output.display()
          ),
          None,
        ))
      }
      Operation::TransferItems { target, source, steamid64 } => {
        let mut target_document = SteamSave::open_path(&target, steamid64)?;
        let source_document = SteamSave::open_path(&source, steamid64)?;
        let report =
          edit::transfer_item_box(target_document.payload_mut(), source_document.payload())?;
        let output = write_edited(&mut target_document, &target, "transferred")?;
        Ok((
          format!(
            "道具箱已转移: {} 槽, {} 件道具\n输出: {}",
            report.slots,
            report.items,
            output.display()
          ),
          None,
        ))
      }
      Operation::TransferEquipment { target, source, steamid64 } => {
        let mut target_document = SteamSave::open_path(&target, steamid64)?;
        let source_document = SteamSave::open_path(&source, steamid64)?;
        let report =
          edit::transfer_equipment(target_document.payload_mut(), source_document.payload())?;
        let output = write_edited(&mut target_document, &target, "transferred")?;
        let class_text = report
          .classes
          .iter()
          .map(|(label, fields)| format!("{label}={fields}字段"))
          .collect::<Vec<_>>()
          .join(", ");
        Ok((
          format!(
            "装备已转移: {class_text};装备箱 {} 槽 {} 件\n输出: {}",
            report.box_slots,
            report.box_used,
            output.display()
          ),
          None,
        ))
      }
      Operation::LoadBox { target, steamid64 } => {
        let document = SteamSave::open_path(&target, steamid64)?;
        let entries = edit::read_item_box(document.payload())?;
        let nonempty = entries.iter().filter(|entry| entry.num > 0).count();
        let items = entries.into_iter().map(|entry| (entry.id, entry.num)).collect();
        Ok((format!("道具箱: 读取完成,{} 件非空", nonempty), Some(items)))
      }
      Operation::SetItems { target, steamid64, changes } => {
        let mut document = SteamSave::open_path(&target, steamid64)?;
        let report = edit::set_item_counts(document.payload_mut(), &changes)?;
        let output = write_edited(&mut document, &target, "edited")?;
        let entries = edit::read_item_box(document.payload())?;
        let items = entries.into_iter().map(|entry| (entry.id, entry.num)).collect();
        Ok((
          format!(
            "道具修改完成: {} 处更新,其中 {} 个新物品填入空槽
输出: {}",
            report.applied,
            report.filled,
            output.display()
          ),
          Some(items),
        ))
      }
    }
  }
}

fn write_edited(
  document: &mut SteamSave,
  target: &PathBuf,
  suffix: &str,
) -> anyhow::Result<PathBuf> {
  let output = derived_output(target, suffix);
  if output == *target {
    anyhow::bail!("输出路径与源文件相同;源存档永远不会被修改");
  }
  document.write_to(&output)?;
  Ok(output)
}

fn derived_output(source: &Path, suffix: &str) -> PathBuf {
  let mut name = source.file_stem().and_then(|stem| stem.to_str()).unwrap_or("save").to_owned();
  name.push('.');
  name.push_str(suffix);
  name.push_str(".bin");
  source.with_file_name(name)
}

/// What a background operation reports: a log line plus, for item box
/// operations, the resulting item box contents as `(item id, count)` pairs.
type OperationOutcome = (String, Option<Vec<(u32, u32)>>);

enum WorkerEvent {
  Finished(OperationOutcome),
}

#[derive(Default)]
pub struct GuiApp {
  target: String,
  source: String,
  steamid64: String,
  money: String,
  points: String,
  busy: bool,
  log: String,
  receiver: Option<Receiver<WorkerEvent>>,
  discovered: Vec<(PathBuf, String)>,
  box_items: Vec<(u32, u32)>,
  box_edits: Vec<String>,
  box_filter: String,
}

impl GuiApp {
  pub fn new(creation_context: &eframe::CreationContext) -> Self {
    let font_status = install_cjk_font(&creation_context.egui_ctx);
    let mut app = Self::default();
    app.log_line(&font_status);
    app.discovered = discover_steam_slot_files();
    if app.discovered.is_empty() {
      app.log_line("自动查找: 未在本机找到 MHR 存档目录,请手动选择存档文件 (见下方帮助)");
    } else {
      app.log_line(&format!(
        "自动查找: 找到 {} 个 MHR 存档目录,可在“快速选择”中选取",
        app.discovered.len()
      ));
    }
    app
  }

  fn log_line(&mut self, text: &str) {
    self.log.push_str(text);
    self.log.push('\n');
  }

  fn spawn(&mut self, ctx: &egui::Context, operation: Operation) {
    let (sender, receiver) = mpsc::channel();
    self.receiver = Some(receiver);
    self.busy = true;
    self.log_line(&format!("▶ {}", operation.describe()));
    let context = ctx.clone();
    thread::spawn(move || {
      let (text, entries) = match operation.run() {
        Ok((text, entries)) => (format!("✔ {text}"), entries),
        Err(error) => (format!("✘ 失败: {error:?}"), None),
      };
      let _ = sender.send(WorkerEvent::Finished((text, entries)));
      context.request_repaint();
    });
  }

  fn parsed_steamid64(&self) -> Result<u64, &'static str> {
    self.steamid64.trim().parse::<u64>().map_err(|_| "SteamID64 必须是纯数字")
  }

  fn pending_item_changes(&self) -> Vec<(u32, u32)> {
    let mut changes = Vec::new();
    if self.box_items.is_empty() || self.box_edits.len() != self.box_items.len() {
      return changes;
    }
    for (index, (id, num)) in self.box_items.iter().enumerate() {
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
}

impl eframe::App for GuiApp {
  fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
    if let Some(receiver) = &self.receiver {
      match receiver.try_recv() {
        Ok(WorkerEvent::Finished((text, entries))) => {
          self.log_line(&text);
          if let Some(entries) = entries {
            self.box_edits = vec![String::new(); entries.len()];
            self.box_items = entries;
          }
          self.busy = false;
          self.receiver = None;
        }
        Err(TryRecvError::Empty) => {}
        Err(TryRecvError::Disconnected) => {
          self.busy = false;
          self.receiver = None;
          self.log_line("✘ 后台线程异常退出");
        }
      }
    }

    egui::CentralPanel::default().show(ctx, |ui| {
      ui.heading("MHRise 存档编辑器 (Steam)");
      ui.label("所有操作输出为新文件,不会修改所选存档;请在游戏关闭后将输出文件重命名替换原存档。");
      ui.add_space(6.0);

      Grid::new("inputs").num_columns(3).spacing([8.0, 6.0]).show(ui, |ui| {
        ui.label("目标存档");
        ui.add_sized([420.0, 20.0], egui::TextEdit::singleline(&mut self.target));
        if ui.add_enabled(!self.busy, egui::Button::new("浏览…")).clicked()
          && let Some(path) = rfd::FileDialog::new()
            .add_filter("MHRise 存档", &["bin"])
            .set_title("选择目标存档")
            .pick_file()
        {
          self.target = path.display().to_string();
        }
        ui.end_row();

        ui.label("来源存档 (转移用)");
        ui.add_sized([420.0, 20.0], egui::TextEdit::singleline(&mut self.source));
        if ui.add_enabled(!self.busy, egui::Button::new("浏览…")).clicked()
          && let Some(path) = rfd::FileDialog::new()
            .add_filter("MHRise 存档", &["bin"])
            .set_title("选择来源存档")
            .pick_file()
        {
          self.source = path.display().to_string();
        }
        ui.end_row();

        ui.label("SteamID64");
        ui.add_sized([420.0, 20.0], egui::TextEdit::singleline(&mut self.steamid64));
        ui.label("账号数字 ID");
        ui.end_row();

        ui.label("快速选择");
        let selected = self
          .discovered
          .iter()
          .find(|(path, _)| path.display().to_string() == self.target)
          .map(|(_, label)| label.clone())
          .unwrap_or_else(|| "(未发现存档)".to_owned());
        egui::ComboBox::from_id_salt("discovered-saves")
          .selected_text(selected)
          .show_ui(ui, |ui| {
            for (path, label) in &self.discovered {
              ui.selectable_value(&mut self.target, path.display().to_string(), label);
            }
          });
        ui.end_row();
      });

      ui.add_space(8.0);
      ui.separator();
      ui.add_space(4.0);

      Grid::new("values").num_columns(4).spacing([8.0, 6.0]).show(ui, |ui| {
        ui.label("金钱");
        ui.add_sized([110.0, 20.0], egui::TextEdit::singleline(&mut self.money));
        let steamid = self.parsed_steamid64();
        let value = self.money.trim().parse::<u32>();
        let response = ui.add_enabled(
          !self.busy && steamid.is_ok() && value.is_ok() && !self.target.trim().is_empty(),
          egui::Button::new("设置金钱"),
        );
        if response.clicked() {
          let operation = Operation::SetMoney {
            target: PathBuf::from(self.target.trim()),
            steamid64: steamid.expect("checked"),
            value: value.expect("checked"),
            total_added: None,
          };
          self.spawn(ctx, operation);
        }
        ui.label("当前余额;累计值不变");
        ui.end_row();

        ui.label("点数");
        ui.add_sized([110.0, 20.0], egui::TextEdit::singleline(&mut self.points));
        let points = self.points.trim().parse::<u32>();
        let response = ui.add_enabled(
          !self.busy && steamid.is_ok() && points.is_ok() && !self.target.trim().is_empty(),
          egui::Button::new("设置点数"),
        );
        if response.clicked() {
          let operation = Operation::SetPoints {
            target: PathBuf::from(self.target.trim()),
            steamid64: steamid.expect("checked"),
            value: points.expect("checked"),
          };
          self.spawn(ctx, operation);
        }
        ui.label("集会所/随从点数");
        ui.end_row();
      });

      ui.add_space(6.0);
      ui.separator();
      ui.add_space(4.0);

      ui.label("转移:来源存档 → 目标存档 (同一 Steam 账号)");
      ui.horizontal(|ui| {
        let steamid = self.parsed_steamid64();
        let ready = !self.busy
          && steamid.is_ok()
          && !self.target.trim().is_empty()
          && !self.source.trim().is_empty();
        if ui.add_enabled(ready, egui::Button::new("转移道具箱")).clicked() {
          let operation = Operation::TransferItems {
            target: PathBuf::from(self.target.trim()),
            source: PathBuf::from(self.source.trim()),
            steamid64: steamid.expect("checked"),
          };
          self.spawn(ctx, operation);
        }
        if ui.add_enabled(ready, egui::Button::new("转移装备+护石+组合")).clicked() {
          let operation = Operation::TransferEquipment {
            target: PathBuf::from(self.target.trim()),
            source: PathBuf::from(self.source.trim()),
            steamid64: steamid.expect("checked"),
          };
          self.spawn(ctx, operation);
        }
        if ui
          .add_enabled(
            !self.busy && steamid.is_ok() && !self.target.trim().is_empty(),
            egui::Button::new("读取存档信息"),
          )
          .clicked()
        {
          let operation = Operation::Inspect {
            target: PathBuf::from(self.target.trim()),
            steamid64: steamid.expect("checked"),
          };
          self.spawn(ctx, operation);
        }
      });

      ui.add_space(6.0);
      ui.separator();
      ui.add_space(4.0);

      ui.label("道具箱编辑 (针对目标存档;先“读取道具箱”,再修改数量)");
      ui.horizontal(|ui| {
        let steamid = self.parsed_steamid64();
        let target_ready = !self.busy && steamid.is_ok() && !self.target.trim().is_empty();
        if ui.add_enabled(target_ready, egui::Button::new("读取道具箱")).clicked() {
          let operation = Operation::LoadBox {
            target: PathBuf::from(self.target.trim()),
            steamid64: steamid.expect("checked"),
          };
          self.spawn(ctx, operation);
        }
        ui.label("筛选");
        ui.add_sized([150.0, 20.0], egui::TextEdit::singleline(&mut self.box_filter));
        let changes = self.pending_item_changes();
        let apply_text = format!("应用修改 ({})", changes.len());
        if ui
          .add_enabled(!self.busy && !changes.is_empty(), egui::Button::new(apply_text))
          .clicked()
        {
          let operation = Operation::SetItems {
            target: PathBuf::from(self.target.trim()),
            steamid64: steamid.expect("checked"),
            changes,
          };
          self.spawn(ctx, operation);
        }
      });
      if !self.box_items.is_empty() {
        let filter = self.box_filter.trim().to_lowercase();
        let rows: Vec<usize> = (0..self.box_items.len())
          .filter(|&index| {
            if filter.is_empty() {
              return true;
            }
            let (id, _) = self.box_items[index];
            let name = crate::items::item_name(id).unwrap_or("");
            name.to_lowercase().contains(&filter)
              || format!("{id:08x}").contains(&filter)
              || format!("{id}").contains(&filter)
          })
          .collect();
        egui::ScrollArea::vertical()
          .max_height(240.0)
          .auto_shrink(false)
          .show_rows(ui, 20.0, rows.len(), |ui, range| {
            for row in range {
              let index = rows[row];
              let (id, num) = self.box_items[index];
              ui.horizontal(|ui| {
                ui.monospace(format!("{id:08x}"));
                let name = crate::items::item_name(id).unwrap_or("(未知物品)");
                ui.add_sized([340.0, 18.0], egui::Label::new(name).truncate());
                ui.add_sized([70.0, 18.0], egui::Label::new(format!("当前 {num}")));
                if num == 0 {
                  ui.label("(空)");
                }
                ui.add_sized([80.0, 18.0], egui::TextEdit::singleline(&mut self.box_edits[index]));
              });
            }
          });
        ui.label(format!(
          "显示 {} / {} 项;“新数量”留空 = 不修改;填 0 = 清空该物品;修改后点“应用修改”",
          rows.len(),
          self.box_items.len()
        ));
      }

      ui.add_space(6.0);
      egui::CollapsingHeader::new("存档在哪里?加载什么文件?")
        .default_open(false)
        .show(ui, |ui| {
          ui.label(r"存档目录: <Steam库>\userdata\<32位账号ID>\1446780\remote\win64_save");
          ui.label(r"· Steam 默认装在 C:\Program Files (x86)\Steam;自定义库在其它盘(例如 D:\Steam)。启动时已自动扫描所有 Steam 库。");
          ui.label("· 角色存档: data001Slot.bin ~ data003Slot.bin(游戏内最多 3 个角色槽,即读档界面的 3 个存档位)");
          ui.label("· 系统存档: data00-1.bin(全局数据,一般不需要修改);SS1_* / SS4_* / SS7_* 是相册数据");
          ui.label("· 推荐流程: backup 备份 → 生成新文件 → 关闭游戏 → 用输出文件替换原存档 → 进游戏确认");
          ui.label("· Steam 云同步: 编辑时建议让 Steam 离线,避免云端旧档覆盖");
        });

      ui.add_space(8.0);
      ui.separator();
      ui.label("运行日志");
      egui::ScrollArea::vertical().stick_to_bottom(true).show(ui, |ui| {
        ui.monospace(if self.log.is_empty() { "(就绪)" } else { &self.log });
      });
    });

    if self.busy {
      ctx.request_repaint_after(Duration::from_millis(120));
    }
  }
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
  roots.push(PathBuf::from(r"C:\Program Files (x86)\Steam"));
  roots.push(PathBuf::from(r"D:\Steam"));
  roots.push(PathBuf::from(r"E:\Steam"));
  roots.sort();
  roots.dedup();

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
  libraries.sort();
  libraries.dedup();

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
  saves.sort();
  saves.dedup();
  saves
}

/// Lists the character slot files inside a discovered save directory.
fn discover_steam_slot_files() -> Vec<(PathBuf, String)> {
  let mut files = Vec::new();
  for dir in discover_save_dirs() {
    for name in ["data001Slot.bin", "data002Slot.bin", "data003Slot.bin"] {
      let path = dir.join(name);
      if path.is_file() {
        let account = dir
          .parent()
          .and_then(|remote| remote.parent())
          .and_then(|appid| appid.parent())
          .and_then(|account| account.file_name())
          .map(|name| name.to_string_lossy().to_string())
          .unwrap_or_default();
        files.push((path, format!("账号 {account} / {name}")));
      }
    }
  }
  files
}
