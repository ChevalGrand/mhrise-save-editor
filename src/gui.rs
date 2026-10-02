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

/// Starts the GUI, loading a CJK-capable system font when available so that
/// Chinese labels render instead of tofu boxes.
pub fn run() -> eframe::Result<()> {
  let options = eframe::NativeOptions {
    viewport: egui::ViewportBuilder::default().with_inner_size([820.0, 660.0]),
    ..Default::default()
  };
  eframe::run_native(
    "MHRise Save Editor",
    options,
    Box::new(|creation_context| {
      let font_status = install_cjk_font(&creation_context.egui_ctx);
      let mut app = GuiApp::default();
      app.log_line(&font_status);
      Ok(Box::new(app))
    }),
  )
}

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
}

impl Operation {
  fn describe(&self) -> String {
    match self {
      Operation::Inspect { .. } => "读取存档信息".to_owned(),
      Operation::SetMoney { value, .. } => format!("设置金钱为 {value}"),
      Operation::SetPoints { value, .. } => format!("设置点数为 {value}"),
      Operation::TransferItems { .. } => "转移道具箱".to_owned(),
      Operation::TransferEquipment { .. } => "转移装备(装备箱+护石+组合)".to_owned(),
    }
  }

  fn run(self) -> anyhow::Result<String> {
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
        Ok(text)
      }
      Operation::SetMoney { target, steamid64, value, total_added } => {
        let mut document = SteamSave::open_path(&target, steamid64)?;
        let report = edit::set_money(document.payload_mut(), value, total_added)?;
        let output = write_edited(&mut document, &target, "edited")?;
        Ok(format!(
          "金钱已设置: {} -> {} (更新 {} 处)\n输出: {}",
          report.before.first().copied().unwrap_or(0),
          value,
          report.updated,
          output.display()
        ))
      }
      Operation::SetPoints { target, steamid64, value } => {
        let mut document = SteamSave::open_path(&target, steamid64)?;
        let report = edit::set_village_points(document.payload_mut(), value)?;
        let output = write_edited(&mut document, &target, "edited")?;
        Ok(format!(
          "点数已设置: {} -> {} (更新 {} 处)\n输出: {}",
          report.before.first().copied().unwrap_or(0),
          value,
          report.updated,
          output.display()
        ))
      }
      Operation::TransferItems { target, source, steamid64 } => {
        let mut target_document = SteamSave::open_path(&target, steamid64)?;
        let source_document = SteamSave::open_path(&source, steamid64)?;
        let report =
          edit::transfer_item_box(target_document.payload_mut(), source_document.payload())?;
        let output = write_edited(&mut target_document, &target, "transferred")?;
        Ok(format!(
          "道具箱已转移: {} 槽, {} 件道具\n输出: {}",
          report.slots,
          report.items,
          output.display()
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
        Ok(format!(
          "装备已转移: {class_text};装备箱 {} 槽 {} 件\n输出: {}",
          report.box_slots,
          report.box_used,
          output.display()
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

enum WorkerEvent {
  Finished(String),
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
}

impl GuiApp {
  pub fn new(_creation_context: &eframe::CreationContext) -> Self {
    Self::default()
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
      let text = match operation.run() {
        Ok(text) => format!("✔ {text}"),
        Err(error) => format!("✘ 失败: {error:?}"),
      };
      let _ = sender.send(WorkerEvent::Finished(text));
      context.request_repaint();
    });
  }

  fn parsed_steamid64(&self) -> Result<u64, &'static str> {
    self.steamid64.trim().parse::<u64>().map_err(|_| "SteamID64 必须是纯数字")
  }
}

impl eframe::App for GuiApp {
  fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
    if let Some(receiver) = &self.receiver {
      match receiver.try_recv() {
        Ok(WorkerEvent::Finished(text)) => {
          self.log_line(&text);
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
