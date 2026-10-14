#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod firewall;
mod server;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

use config::Config;

const BG: egui::Color32 = egui::Color32::from_rgb(245, 247, 250);
const SURFACE: egui::Color32 = egui::Color32::from_rgb(255, 255, 255);
const FIELD: egui::Color32 = egui::Color32::from_rgb(236, 240, 245);
const LINE: egui::Color32 = egui::Color32::from_rgb(210, 218, 228);
const TEXT: egui::Color32 = egui::Color32::from_rgb(28, 34, 44);
const MUTED: egui::Color32 = egui::Color32::from_rgb(110, 122, 138);
const ACCENT: egui::Color32 = egui::Color32::from_rgb(46, 132, 196);
const MOSS: egui::Color32 = egui::Color32::from_rgb(36, 140, 100);
const CLAY: egui::Color32 = egui::Color32::from_rgb(196, 72, 64);
const AMBER: egui::Color32 = egui::Color32::from_rgb(180, 120, 32);
const DISABLED: egui::Color32 = egui::Color32::from_rgb(220, 226, 234);
const INK: egui::Color32 = egui::Color32::from_rgb(20, 24, 30);

fn main() {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("局域网文件分享")
            .with_inner_size([720.0, 360.0])
            .with_min_inner_size([640.0, 320.0])
            .with_icon(app_icon()),
        ..Default::default()
    };
    if let Err(err) = eframe::run_native(
        "局域网文件分享",
        options,
        Box::new(|cc| Ok(Box::new(LanShareApp::new(cc)))),
    ) {
        eprintln!("无法打开窗口：{err}");
        std::process::exit(1);
    }
}

enum Phase {
    Stopped,
    Starting,
    Running,
    Stopping,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Settings,
    Qr,
}

enum ServerEvent {
    Listening { urls: Vec<String>, port: u16 },
    Failed(String),
    Stopped { removed_rule: bool },
}

struct LanShareApp {
    port: u16,
    port_text: String,
    paths: Vec<PathBuf>,
    path_input: String,
    phase: Phase,
    status: String,
    toast: String,
    urls: Vec<String>,
    primary_url: String,
    share_labels: Vec<(String, PathBuf)>,
    page: Page,
    qr_notice: String,
    qr: Option<egui::TextureHandle>,
    qr_for: String,
    events: Receiver<ServerEvent>,
    events_tx: Sender<ServerEvent>,
    stop_flag: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl LanShareApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install_fonts(&cc.egui_ctx);
        install_style(&cc.egui_ctx);
        let config = Config::load();
        let status = if config.paths.is_empty() {
            "请先选择要分享的文件夹。未选择前不会使用当前目录。".into()
        } else {
            if cfg!(windows) {
                "已停止。点击开始后会放行防火墙并提供下载。"
            } else {
                "已停止。点击开始后即可在局域网下载。"
            }
            .into()
        };
        let page = if config.paths.is_empty() {
            Page::Settings
        } else {
            Page::Qr
        };
        let (events_tx, events) = mpsc::channel();
        Self {
            port: config.port,
            port_text: config.port.to_string(),
            paths: config.paths,
            path_input: String::new(),
            phase: Phase::Stopped,
            status,
            toast: String::new(),
            urls: Vec::new(),
            primary_url: String::new(),
            share_labels: Vec::new(),
            page,
            qr_notice: String::new(),
            qr: None,
            qr_for: String::new(),
            events,
            events_tx,
            stop_flag: Arc::new(AtomicBool::new(false)),
            worker: None,
        }
    }

    fn editable(&self) -> bool {
        matches!(self.phase, Phase::Stopped)
    }

    fn save(&mut self) {
        let config = Config {
            port: self.port,
            paths: self.paths.clone(),
        };
        if let Err(err) = config.save() {
            self.toast = err;
        }
    }

    fn commit_port(&mut self) {
        match self.port_text.trim().parse::<u16>() {
            Ok(port) if port > 0 => self.port = port,
            _ => self.port_text = self.port.to_string(),
        }
    }

    fn add_path(&mut self, path: PathBuf) -> bool {
        if !path.exists() {
            self.status = format!("路径不存在：{}", path.display());
            return false;
        }
        if self
            .paths
            .iter()
            .any(|existing| server::paths_equal(existing, &path))
        {
            self.status = "这个路径已经在列表里。".into();
            return false;
        }
        self.paths.push(path);
        self.save();
        self.status = "已添加路径。".into();
        true
    }

    fn start(&mut self) {
        if !self.editable() {
            return;
        }
        self.commit_port();
        self.port_text = self.port.to_string();
        if self.paths.is_empty() {
            self.status = "请至少添加一个路径。".into();
            return;
        }
        if let Err(err) = server::prepare_shares(&self.paths) {
            self.status = err;
            return;
        }
        self.save();
        self.stop_flag.store(false, Ordering::SeqCst);
        self.phase = Phase::Starting;
        self.status = if cfg!(windows) {
            format!("正在放行防火墙 TCP {}，并启动服务…", self.port)
        } else {
            format!("正在启动服务，端口 {}…", self.port)
        };
        self.toast.clear();
        let port = self.port;
        let paths = self.paths.clone();
        let stop = Arc::clone(&self.stop_flag);
        let tx = self.events_tx.clone();
        match std::thread::Builder::new()
            .name("lan-share-http".into())
            .spawn(move || run_worker(port, paths, stop, tx))
        {
            Ok(handle) => self.worker = Some(handle),
            Err(err) => {
                self.phase = Phase::Stopped;
                self.status = format!("无法启动服务线程：{err}");
            }
        }
    }

    fn stop(&mut self) {
        if !matches!(self.phase, Phase::Starting | Phase::Running) {
            return;
        }
        self.phase = Phase::Stopping;
        self.stop_flag.store(true, Ordering::SeqCst);
        self.status = if cfg!(windows) {
            "正在停止服务。如果弹出授权窗口，请允许它移除防火墙规则。"
        } else {
            "正在停止服务…"
        }
        .into();
    }

    fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(event) = self.events.try_recv() {
            self.apply(event);
        }
        if self
            .worker
            .as_ref()
            .is_some_and(|handle| handle.is_finished())
        {
            if let Some(handle) = self.worker.take() {
                let _ = handle.join();
            }
            if !matches!(self.phase, Phase::Stopped) {
                self.phase = Phase::Stopped;
                self.urls.clear();
                self.clear_qr();
                self.status = "服务线程已结束。".into();
            }
        }
        if !matches!(self.phase, Phase::Stopped) {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
    }

    fn apply(&mut self, event: ServerEvent) {
        match event {
            ServerEvent::Listening { urls, port } => {
                self.primary_url = urls.first().cloned().unwrap_or_default();
                self.urls = urls;
                self.page = Page::Qr;
                self.qr = None;
                self.qr_for.clear();
                self.qr_notice.clear();
                self.share_labels = server::prepare_shares(&self.paths)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|share| (share.id, share.label_path))
                    .collect();
                self.phase = Phase::Running;
                self.status = if cfg!(windows) {
                    format!("服务已启动，防火墙已放行 TCP {port}。同一局域网设备可直接访问。")
                } else {
                    format!("服务已启动，端口 {port}。同一局域网设备可直接访问。")
                };
            }
            ServerEvent::Failed(message) => {
                self.phase = Phase::Stopped;
                self.urls.clear();
                self.share_labels.clear();
                self.clear_qr();
                self.status = message;
            }
            ServerEvent::Stopped { removed_rule } => {
                self.phase = Phase::Stopped;
                self.urls.clear();
                self.share_labels.clear();
                self.clear_qr();
                self.status = if removed_rule {
                    "已停止，并移除了本次添加的防火墙规则。".into()
                } else {
                    "已停止。".into()
                };
            }
        }
    }

    fn clear_qr(&mut self) {
        self.qr = None;
        self.qr_for.clear();
        self.qr_notice.clear();
        if self.paths.is_empty() {
            self.page = Page::Settings;
        }
    }

    fn ensure_qr(&mut self, ctx: &egui::Context) {
        if self.primary_url.is_empty() || self.qr_for == self.primary_url {
            return;
        }
        let Some(image) = qr_image(&self.primary_url) else {
            self.qr = None;
            self.qr_for = self.primary_url.clone();
            return;
        };
        self.qr = Some(ctx.load_texture("lan-share-qr", image, egui::TextureOptions::NEAREST));
        self.qr_for = self.primary_url.clone();
    }
}

impl Drop for LanShareApp {
    fn drop(&mut self) {
        self.stop_flag.store(true, Ordering::SeqCst);
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
        self.commit_port();
        let _ = Config {
            port: self.port,
            paths: self.paths.clone(),
        }
        .save();
    }
}

impl eframe::App for LanShareApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll(ctx);
        let mut dropped = Vec::new();
        ctx.input(|input| {
            for file in &input.raw.dropped_files {
                if let Some(path) = &file.path {
                    dropped.push(path.clone());
                }
            }
        });
        if !dropped.is_empty() {
            if self.editable() {
                for path in dropped {
                    self.add_path(path);
                }
            } else {
                self.toast = "请先停止服务，再更改分享路径。".into();
            }
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG).inner_margin(12))
            .show(ctx, |ui| {
                self.draw(ui, ctx);
            });
    }
}

impl LanShareApp {
    fn draw(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Title("局域网文件分享".into()));
        ui.spacing_mut().item_spacing.y = 6.0;
        self.draw_topbar(ui);
        ui.add_space(4.0);
        match self.page {
            Page::Settings => self.draw_console(ui, ctx),
            Page::Qr => self.draw_qr_page(ui, ctx),
        }
    }

    fn draw_topbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("局域网文件分享").size(18.0).color(TEXT));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                match self.page {
                    Page::Settings => {
                        let can_open_qr = matches!(self.phase, Phase::Running) && !self.urls.is_empty();
                        if back_to_qr_button(ui, can_open_qr).clicked() {
                            self.page = Page::Qr;
                        }
                    }
                    Page::Qr => {
                        if settings_button(ui).clicked() {
                            self.page = Page::Settings;
                        }
                    }
                }
                let on_qr = self.page == Page::Qr;
                if status_pill(ui, &self.phase, on_qr).clicked() {
                    self.stop();
                }
            });
        });
    }

    fn draw_console(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let mut browse = false;
        let mut add = false;
        let mut remove_at = None;
        let editable = self.editable();

        ui.horizontal(|ui| {
            ui.label("端口");
            let port_edit = ui.add_enabled(
                editable,
                egui::TextEdit::singleline(&mut self.port_text)
                    .desired_width(72.0)
                    .hint_text("8000")
                    .font(egui::TextStyle::Monospace),
            );
            if port_edit.lost_focus() {
                self.commit_port();
                self.port_text = self.port.to_string();
                self.save();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let can_stop = matches!(self.phase, Phase::Starting | Phase::Running);
                let can_start = editable && !self.paths.is_empty();
                if action_button(
                    ui,
                    "停止",
                    CLAY,
                    egui::Color32::from_rgb(255, 246, 242),
                    can_stop,
                )
                .clicked()
                {
                    self.stop();
                }
                let start = action_button(
                    ui,
                    "开始",
                    MOSS,
                    egui::Color32::from_rgb(243, 255, 246),
                    can_start,
                )
                .on_hover_text(start_hint());
                if start.clicked() {
                    self.start();
                }
            });
        });

        if self.paths.is_empty() {
            ui.label(
                egui::RichText::new("请选择要分享的文件夹。未选择前不会使用当前目录。")
                    .color(ACCENT)
                    .size(14.0),
            );
        } else if self.paths.len() > 3 {
            let paths = self.paths.clone();
            egui::ScrollArea::vertical()
                .id_salt("share-paths")
                .max_height(72.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    draw_path_rows(ui, &paths, editable, &mut remove_at);
                });
        } else {
            draw_path_rows(ui, &self.paths, editable, &mut remove_at);
        }

        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(editable, egui::Button::new("添加"))
                    .clicked()
                {
                    add = true;
                }
                if self.paths.is_empty() {
                    if ui
                        .add_enabled(
                            editable,
                            egui::Button::new(
                                egui::RichText::new("选择文件夹").color(egui::Color32::WHITE),
                            )
                            .fill(ACCENT),
                        )
                        .clicked()
                    {
                        browse = true;
                    }
                } else if ui
                    .add_enabled(editable, egui::Button::new("浏览"))
                    .clicked()
                {
                    browse = true;
                }
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    let input = ui.add_enabled(
                        editable,
                        egui::TextEdit::singleline(&mut self.path_input)
                            .desired_width(ui.available_width())
                            .hint_text("输入路径，或把文件夹拖进窗口"),
                    );
                    if input.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                        add = true;
                    }
                });
            });
        });

        ui.label(egui::RichText::new(&self.status).color(TEXT).size(13.0));
        if !self.toast.is_empty() {
            ui.label(egui::RichText::new(&self.toast).color(ACCENT).size(13.0));
        }

        if !self.urls.is_empty() {
            ui.label(egui::RichText::new("地址").color(MUTED).size(13.0));
            let urls = self.urls.clone();
            let labels = self.share_labels.clone();
            egui::ScrollArea::vertical()
                .id_salt("urls")
                .max_height(ui.available_height())
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for url in urls {
                        ui.horizontal(|ui| {
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.button("打开").clicked() {
                                        open_browser(&url);
                                    }
                                    if ui.button("复制").clicked() {
                                        ctx.copy_text(url.clone());
                                        self.toast = format!("已复制 {url}");
                                    }
                                    ui.with_layout(
                                        egui::Layout::left_to_right(egui::Align::Center),
                                        |ui| {
                                            ui.add(
                                                egui::Label::new(
                                                    egui::RichText::new(&url)
                                                        .family(egui::FontFamily::Monospace)
                                                        .size(14.0)
                                                        .color(TEXT),
                                                )
                                                .truncate(),
                                            );
                                        },
                                    );
                                },
                            );
                        });
                    }
                    if labels.len() > 1 {
                        for (id, path) in &labels {
                            ui.label(
                                egui::RichText::new(format!("/{id}/  {}", path.display()))
                                    .family(egui::FontFamily::Monospace)
                                    .size(12.0)
                                    .color(MUTED),
                            );
                        }
                    }
                });
        }

        if let Some(index) = remove_at {
            self.paths.remove(index);
            self.save();
            self.status = "已移除路径。".into();
            if self.paths.is_empty() {
                self.page = Page::Settings;
            }
        }
        if browse {
            if let Some(path) = rfd::FileDialog::new()
                .set_title("选择要分享的文件夹")
                .pick_folder()
            {
                self.add_path(path);
            }
        }
        if add {
            let text = self.path_input.trim().trim_matches('"').trim();
            if text.is_empty() {
                self.status = "请先输入路径。".into();
            } else if self.add_path(PathBuf::from(text)) {
                self.path_input.clear();
            }
        }
    }

    fn draw_qr_page(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if self.urls.is_empty() {
            self.draw_qr_idle(ui);
            return;
        }
        self.draw_qr_body(ui, ctx);
    }

    fn draw_qr_idle(&mut self, ui: &mut egui::Ui) {
        let can_start = self.editable() && !self.paths.is_empty();
        let (hint, show_start) = match self.phase {
            Phase::Stopped => {
                if self.paths.is_empty() {
                    (
                        "请先点右上角齿轮进入设置，选择要分享的文件夹。",
                        false,
                    )
                } else {
                    ("服务已停止。点击下方按钮开始分享，二维码会显示在这里。", true)
                }
            }
            Phase::Starting => ("正在启动服务，可点右上角状态停止。", false),
            Phase::Stopping => ("正在停止服务，请稍候…", false),
            Phase::Running => ("正在准备二维码…", false),
        };

        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.18);
            ui.label(egui::RichText::new(hint).color(MUTED).size(15.0));
            if show_start {
                ui.add_space(18.0);
                let start = ui
                    .add_enabled(
                        can_start,
                        egui::Button::new(
                            egui::RichText::new("开始")
                                .size(22.0)
                                .color(if can_start {
                                    egui::Color32::from_rgb(243, 255, 246)
                                } else {
                                    MUTED
                                }),
                        )
                        .fill(if can_start { MOSS } else { DISABLED })
                        .min_size(egui::vec2(180.0, 52.0))
                        .corner_radius(12),
                    )
                    .on_hover_text(start_hint());
                if start.clicked() {
                    self.start();
                }
            }
        });
    }

    fn draw_qr_body(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.ensure_qr(ctx);
        ui.horizontal_top(|ui| {
            egui::Frame::new()
                .fill(egui::Color32::WHITE)
                .corner_radius(12)
                .inner_margin(10)
                .show(ui, |ui| {
                    if let Some(texture) = &self.qr {
                        ui.add(
                            egui::Image::new(texture).fit_to_exact_size(egui::vec2(200.0, 200.0)),
                        );
                    } else {
                        ui.label(egui::RichText::new("无法生成二维码").color(MUTED));
                    }
                });
            ui.vertical(|ui| {
                ui.label(egui::RichText::new("用手机扫描").color(MUTED).size(13.0));
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(&self.primary_url)
                            .family(egui::FontFamily::Monospace)
                            .size(14.0)
                            .color(TEXT),
                    )
                    .truncate(),
                );
                ui.label(
                    egui::RichText::new("微信扫码后，请点右上角 ··· 用浏览器打开")
                        .color(CLAY)
                        .size(13.0),
                );
                ui.horizontal(|ui| {
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new("复制")
                                    .size(14.0)
                                    .color(egui::Color32::WHITE),
                            )
                            .fill(ACCENT),
                        )
                        .clicked()
                    {
                        ctx.copy_text(self.primary_url.clone());
                        self.qr_notice = "已复制".into();
                    }
                    if ui.button("打开").clicked() {
                        open_browser(&self.primary_url);
                    }
                    if !self.qr_notice.is_empty() {
                        ui.label(
                            egui::RichText::new(&self.qr_notice)
                                .color(ACCENT)
                                .size(13.0),
                        );
                    }
                });
                if self.urls.len() > 1 {
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new("地址").color(MUTED).size(13.0));
                    let mut selected = self.primary_url.clone();
                    for url in self.urls.clone() {
                        let label = url
                            .trim_start_matches("http://")
                            .trim_end_matches('/')
                            .to_string();
                        ui.radio_value(&mut selected, url, label);
                    }
                    if selected != self.primary_url {
                        self.primary_url = selected;
                        self.qr_notice.clear();
                        self.ensure_qr(ctx);
                    }
                }
            });
        });
    }
}

fn draw_path_rows(
    ui: &mut egui::Ui,
    paths: &[std::path::PathBuf],
    editable: bool,
    remove_at: &mut Option<usize>,
) {
    for (index, path) in paths.iter().enumerate() {
        let missing = !path.exists();
        let mut text = path.display().to_string();
        if missing {
            text.push_str("  （不存在）");
        }
        egui::Frame::new()
            .fill(FIELD)
            .stroke(egui::Stroke::new(1.0_f32, LINE))
            .corner_radius(6)
            .inner_margin(egui::vec2(8.0, 4.0))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(editable, egui::Button::new("移除"))
                            .clicked()
                        {
                            *remove_at = Some(index);
                        }
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(text)
                                        .family(egui::FontFamily::Monospace)
                                        .size(14.0)
                                        .color(if missing { CLAY } else { TEXT }),
                                )
                                .truncate(),
                            );
                        });
                    });
                });
            });
    }
}

fn back_to_qr_button(ui: &mut egui::Ui, enabled: bool) -> egui::Response {
    let response = ui
        .add_enabled(
            enabled,
            egui::Button::new(
                egui::RichText::new("← 二维码")
                    .size(14.0)
                    .color(if enabled {
                        egui::Color32::WHITE
                    } else {
                        MUTED
                    }),
            )
            .fill(if enabled { ACCENT } else { DISABLED })
            .min_size(egui::vec2(96.0, 32.0))
            .corner_radius(8),
        );
    if enabled {
        response.on_hover_text("返回二维码页面")
    } else {
        response.on_hover_text("服务启动后才能查看二维码")
    }
}

fn settings_button(ui: &mut egui::Ui) -> egui::Response {
    let (rect, mut response) = ui.allocate_exact_size(egui::vec2(32.0, 32.0), egui::Sense::click());
    response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    let fill = if response.hovered() || response.is_pointer_button_down_on() {
        FIELD
    } else {
        SURFACE
    };
    if ui.is_rect_visible(rect) {
        ui.painter().rect_filled(rect, 8.0, fill);
        ui.painter().rect_stroke(
            rect,
            8.0,
            egui::Stroke::new(1.0_f32, LINE),
            egui::StrokeKind::Inside,
        );
        paint_gear(ui.painter(), rect.center(), TEXT, fill);
    }
    response.on_hover_text("设置")
}

fn paint_gear(
    painter: &egui::Painter,
    center: egui::Pos2,
    color: egui::Color32,
    hole: egui::Color32,
) {
    let stroke = egui::Stroke::new(1.7_f32, color);
    painter.circle_stroke(center, 5.2, stroke);
    for step in 0..8 {
        let angle = step as f32 * std::f32::consts::TAU / 8.0;
        let direction = egui::vec2(angle.cos(), angle.sin());
        painter.line_segment([center + direction * 4.6, center + direction * 8.0], stroke);
    }
    painter.circle_filled(center, 2.1, hole);
    painter.circle_stroke(center, 2.1, stroke);
}

fn status_pill(ui: &mut egui::Ui, phase: &Phase, stoppable: bool) -> egui::Response {
    let can_stop = stoppable && matches!(phase, Phase::Starting | Phase::Running);
    let (base_fill, text, label, stroke) = if can_stop {
        (
            egui::Color32::from_rgb(255, 236, 232),
            CLAY,
            "停止",
            CLAY,
        )
    } else {
        match phase {
            Phase::Stopped => (FIELD, MUTED, "已停止", LINE),
            Phase::Starting => (egui::Color32::from_rgb(255, 244, 220), AMBER, "启动中", LINE),
            Phase::Running => (
                egui::Color32::from_rgb(226, 245, 236),
                egui::Color32::from_rgb(24, 120, 86),
                "运行中",
                LINE,
            ),
            Phase::Stopping => (egui::Color32::from_rgb(255, 244, 220), AMBER, "停止中", LINE),
        }
    };

    let text_galley = ui.fonts(|fonts| {
        fonts.layout_no_wrap(
            format!("●  {label}"),
            egui::FontId::proportional(13.0),
            text,
        )
    });
    let padding = egui::vec2(10.0, 4.0);
    let size = text_galley.size() + 2.0 * padding;
    let sense = if can_stop {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    let fill = if can_stop && response.hovered() {
        egui::Color32::from_rgb(248, 220, 214)
    } else {
        base_fill
    };
    if ui.is_rect_visible(rect) {
        ui.painter().rect_filled(rect, 14.0, fill);
        ui.painter().rect_stroke(
            rect,
            14.0,
            egui::Stroke::new(1.0_f32, stroke),
            egui::StrokeKind::Inside,
        );
        ui.painter().galley(
            egui::pos2(
                rect.min.x + padding.x,
                rect.center().y - text_galley.size().y * 0.5,
            ),
            text_galley,
            text,
        );
    }
    if can_stop {
        response.on_hover_text(if cfg!(windows) {
            "点击停止服务，并移除本次添加的防火墙规则。"
        } else {
            "点击停止服务。"
        })
    } else {
        response
    }
}

fn action_button(
    ui: &mut egui::Ui,
    label: &str,
    fill: egui::Color32,
    text: egui::Color32,
    enabled: bool,
) -> egui::Response {
    let fill = if enabled { fill } else { DISABLED };
    let text = if enabled { text } else { MUTED };
    ui.add_enabled(
        enabled,
        egui::Button::new(egui::RichText::new(label).size(15.0).color(text))
            .fill(fill)
            .min_size(egui::vec2(84.0, 30.0)),
    )
}

fn run_worker(port: u16, paths: Vec<PathBuf>, stop: Arc<AtomicBool>, tx: Sender<ServerEvent>) {
    let mut session = match firewall::FirewallSession::open(port) {
        Ok(session) => session,
        Err(err) => {
            let _ = tx.send(ServerEvent::Failed(err));
            return;
        }
    };
    if stop.load(Ordering::SeqCst) {
        let removed_rule = session.created();
        let _ = session.close();
        let _ = tx.send(ServerEvent::Stopped { removed_rule });
        return;
    }
    let result = serve_until_stop(port, &paths, &stop, &tx);
    let removed_rule = session.created();
    let closed = session.close();
    match (result, closed) {
        (Err(err), _) => {
            let _ = tx.send(ServerEvent::Failed(err));
        }
        (Ok(()), Err(err)) => {
            let _ = tx.send(ServerEvent::Failed(err));
        }
        (Ok(()), Ok(())) => {
            let _ = tx.send(ServerEvent::Stopped { removed_rule });
        }
    }
}

fn serve_until_stop(
    port: u16,
    paths: &[PathBuf],
    stop: &AtomicBool,
    tx: &Sender<ServerEvent>,
) -> Result<(), String> {
    let shares = server::prepare_shares(paths)?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|err| format!("无法创建运行时：{err}"))?;
    runtime.block_on(async {
        let listener = tokio::net::TcpListener::bind(SocketAddr::from(([0, 0, 0, 0], port)))
            .await
            .map_err(|err| bind_error(err, port))?;
        let _ = tx.send(ServerEvent::Listening {
            urls: server::lan_urls(port),
            port,
        });
        let app = server::router(shares);
        let serve = std::future::IntoFuture::into_future(axum::serve(listener, app));
        tokio::pin!(serve);
        loop {
            tokio::select! {
                result = &mut serve => {
                    return match result {
                        Ok(()) => Ok(()),
                        Err(err) => Err(format!("服务中断：{err}")),
                    };
                }
                _ = tokio::time::sleep(Duration::from_millis(150)) => {
                    if stop.load(Ordering::SeqCst) {
                        return Ok(());
                    }
                }
            }
        }
    })
}

fn bind_error(err: std::io::Error, port: u16) -> String {
    const WSAEADDRINUSE: i32 = 10048;
    const WSAEACCES: i32 = 10013;
    if err.kind() == std::io::ErrorKind::AddrInUse || err.raw_os_error() == Some(WSAEADDRINUSE) {
        format!("端口 {port} 已被占用，请换一个端口。")
    } else if err.kind() == std::io::ErrorKind::PermissionDenied
        || err.raw_os_error() == Some(WSAEACCES)
    {
        format!("没有权限监听端口 {port}。")
    } else {
        format!("无法监听 0.0.0.0:{port}：{err}")
    }
}

fn qr_image(text: &str) -> Option<egui::ColorImage> {
    let code = qrcode::QrCode::new(text.as_bytes()).ok()?;
    let width = code.width();
    let scale = 6;
    let quiet = 3;
    let side = (width + quiet * 2) * scale;
    let mut pixels = vec![egui::Color32::WHITE; side * side];
    let ink = INK;
    for y in 0..width {
        for x in 0..width {
            if code[(x, y)] != qrcode::Color::Dark {
                continue;
            }
            for dy in 0..scale {
                for dx in 0..scale {
                    let px = (x + quiet) * scale + dx;
                    let py = (y + quiet) * scale + dy;
                    pixels[py * side + px] = ink;
                }
            }
        }
    }
    Some(egui::ColorImage {
        size: [side, side],
        pixels,
    })
}

fn install_fonts(ctx: &egui::Context) {
    let Some((bytes, index)) = load_system_cjk_font() else {
        return;
    };
    let mut font = egui::FontData::from_owned(bytes);
    font.index = index;
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert("cjk".to_owned(), Arc::new(font));
    if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
        family.insert(0, "cjk".to_owned());
    }
    if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
        family.push("cjk".to_owned());
    }
    ctx.set_fonts(fonts);
}

fn load_system_cjk_font() -> Option<(Vec<u8>, u32)> {
    for path in cjk_font_paths() {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        if let Some(index) = cjk_face_index(&bytes) {
            return Some((bytes, index));
        }
    }
    None
}

fn cjk_font_paths() -> &'static [&'static str] {
    if cfg!(windows) {
        &[
            r"C:\Windows\Fonts\Deng.ttf",
            r"C:\Windows\Fonts\simhei.ttf",
            r"C:\Windows\Fonts\simfang.ttf",
        ]
    } else if cfg!(target_os = "macos") {
        &[
            "/System/Library/Fonts/PingFang.ttc",
            "/System/Library/Fonts/Hiragino Sans GB.ttc",
            "/System/Library/Fonts/STHeiti Medium.ttc",
            "/System/Library/Fonts/Supplemental/Songti.ttc",
        ]
    } else {
        &[]
    }
}

/// Pick a Simplified Chinese face. Collection offsets are from the start of the file.
/// Face 0 of PingFang.ttc is PingFang HK, which is not the face we want.
fn cjk_face_index(data: &[u8]) -> Option<u32> {
    if data.len() < 12 {
        return None;
    }
    if &data[..4] != b"ttcf" {
        return Some(0);
    }
    let count = read_u32(data, 8)?.min(64);
    let mut best: Option<(i32, u32)> = None;
    for index in 0..count {
        let Some(offset) = read_u32(data, 12 + index as usize * 4) else {
            continue;
        };
        let score = face_score(data, offset as usize);
        if score > 0 && best.map(|(prev, _)| score > prev).unwrap_or(true) {
            best = Some((score, index));
        }
    }
    best.map(|(_, index)| index)
}

fn face_score(data: &[u8], font_offset: usize) -> i32 {
    let num_tables = read_u16(data, font_offset + 4).unwrap_or(0) as usize;
    for table in 0..num_tables {
        let record = font_offset + 12 + table * 16;
        let Some(tag) = data.get(record..record + 4) else {
            break;
        };
        if tag != b"name" {
            continue;
        }
        let Some(offset) = read_u32(data, record + 8) else {
            break;
        };
        let Some(length) = read_u32(data, record + 12) else {
            break;
        };
        let start = offset as usize;
        let end = start.saturating_add(length as usize);
        let Some(table) = data.get(start..end) else {
            break;
        };
        return name_table_score(table);
    }
    0
}

fn name_table_score(table: &[u8]) -> i32 {
    let count = read_u16(table, 2).unwrap_or(0) as usize;
    let string_offset = read_u16(table, 4).unwrap_or(0) as usize;
    let mut best = 0;
    for index in 0..count {
        let record = 6 + index * 12;
        let platform = read_u16(table, record).unwrap_or(0);
        let name_id = read_u16(table, record + 6).unwrap_or(0);
        if platform != 3 || (name_id != 1 && name_id != 16) {
            continue;
        }
        let length = read_u16(table, record + 8).unwrap_or(0) as usize;
        let offset = read_u16(table, record + 10).unwrap_or(0) as usize;
        let start = string_offset + offset;
        let Some(raw) = table.get(start..start + length) else {
            continue;
        };
        let Ok(name) = decode_utf16_be(raw) else {
            continue;
        };
        best = best.max(score_font_name(&name));
    }
    best
}

fn score_font_name(name: &str) -> i32 {
    let name = name.trim();
    if name.starts_with('.') {
        return 0;
    }
    let lower = name.to_ascii_lowercase();
    let mut score = 0;
    if name.contains('简') {
        score += 100;
    }
    if lower.contains(" sc") || lower.ends_with("sc") {
        score += 80;
    }
    if lower.contains("sans gb") || lower.contains(" gb") {
        score += 60;
    }
    if name.contains('繁') || name.contains('港') {
        score -= 100;
    }
    if lower.contains(" tc")
        || lower.ends_with("tc")
        || lower.contains(" hk")
        || lower.ends_with("hk")
    {
        score -= 100;
    }
    score
}

fn decode_utf16_be(raw: &[u8]) -> Result<String, ()> {
    if raw.len() % 2 != 0 {
        return Err(());
    }
    let units: Vec<u16> = raw
        .chunks_exact(2)
        .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
        .collect();
    String::from_utf16(&units).map_err(|_| ())
}

fn read_u16(data: &[u8], offset: usize) -> Option<u16> {
    let bytes = data.get(offset..offset + 2)?;
    Some(u16::from_be_bytes([bytes[0], bytes[1]]))
}

fn read_u32(data: &[u8], offset: usize) -> Option<u32> {
    let bytes = data.get(offset..offset + 4)?;
    Some(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn install_style(ctx: &egui::Context) {
    ctx.set_theme(egui::ThemePreference::Light);
    let mut visuals = egui::Visuals::light();
    visuals.window_fill = BG;
    visuals.panel_fill = BG;
    visuals.faint_bg_color = FIELD;
    visuals.extreme_bg_color = SURFACE;
    visuals.code_bg_color = FIELD;
    visuals.hyperlink_color = ACCENT;
    visuals.selection.bg_fill = egui::Color32::from_rgb(210, 232, 248);
    visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0_f32, LINE);
    visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0_f32, TEXT);
    visuals.widgets.inactive.bg_fill = SURFACE;
    visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0_f32, LINE);
    visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0_f32, TEXT);
    visuals.widgets.inactive.weak_bg_fill = FIELD;
    visuals.widgets.hovered.bg_fill = FIELD;
    visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0_f32, ACCENT);
    visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.0_f32, TEXT);
    visuals.widgets.active.bg_fill = ACCENT;
    visuals.widgets.active.fg_stroke = egui::Stroke::new(1.0_f32, egui::Color32::WHITE);
    visuals.widgets.open.bg_fill = SURFACE;
    ctx.set_visuals_of(egui::Theme::Light, visuals.clone());
    ctx.set_visuals_of(egui::Theme::Dark, visuals);

    ctx.style_mut(|style| {
        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.button_padding = egui::vec2(12.0, 6.0);
        style.text_styles.insert(
            egui::TextStyle::Small,
            egui::FontId::new(13.0, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Body,
            egui::FontId::new(16.0, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Button,
            egui::FontId::new(15.0, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Heading,
            egui::FontId::new(26.0, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Monospace,
            egui::FontId::new(14.0, egui::FontFamily::Monospace),
        );
    });
}

fn app_icon() -> egui::IconData {
    let image = image::load_from_memory(include_bytes!("../assets/icon.png"))
        .expect("icon.png")
        .into_rgba8();
    let (width, height) = image.dimensions();
    egui::IconData {
        rgba: image.into_raw(),
        width,
        height,
    }
}

#[cfg(windows)]
fn open_browser(url: &str) {
    use std::os::windows::process::CommandExt;
    let _ = std::process::Command::new("cmd")
        .args(["/C", "start", "", url])
        .creation_flags(0x08000000)
        .spawn();
}

#[cfg(target_os = "macos")]
fn open_browser(url: &str) {
    let _ = std::process::Command::new("open").arg(url).spawn();
}

#[cfg(not(any(windows, target_os = "macos")))]
fn open_browser(url: &str) {
    let _ = url;
}

fn start_hint() -> &'static str {
    if cfg!(windows) {
        "开始时会请求管理员权限放行防火墙，停止时移除本次添加的规则。"
    } else {
        "开始后，同一局域网内的设备可以用浏览器访问。"
    }
}

#[cfg(test)]
mod font_tests {
    use super::{cjk_face_index, score_font_name};

    #[test]
    fn prefers_simplified_family_names() {
        assert!(score_font_name("PingFang SC") > 0);
        assert!(score_font_name("苹方-简") > 0);
        assert!(score_font_name("Hiragino Sans GB") > 0);
        assert!(score_font_name("Heiti SC") > 0);
        assert!(score_font_name("PingFang HK") < 0);
        assert!(score_font_name("PingFang TC") < 0);
        assert!(score_font_name("苹方-港") < 0);
        assert_eq!(score_font_name(".PingFang SC"), 0);
    }

    #[test]
    fn pingfang_collection_uses_simplified_face() {
        let path = "/System/Library/Fonts/PingFang.ttc";
        if !std::path::Path::new(path).exists() {
            return;
        }
        let bytes = std::fs::read(path).unwrap();
        assert_eq!(cjk_face_index(&bytes), Some(2));
    }

    #[test]
    fn system_cjk_font_draws_ui_text() {
        let Some((bytes, index)) = super::load_system_cjk_font() else {
            return;
        };
        let mut font = egui::FontData::from_owned(bytes);
        font.index = index;
        let mut fonts = egui::FontDefinitions::default();
        fonts
            .font_data
            .insert("cjk".to_owned(), std::sync::Arc::new(font));
        fonts
            .families
            .get_mut(&egui::FontFamily::Proportional)
            .unwrap()
            .insert(0, "cjk".to_owned());
        let mut fonts = egui::epaint::text::FontsImpl::new(1.0, 2048, fonts);
        assert!(fonts.has_glyphs(
            &egui::FontId::proportional(16.0),
            "局域网文件分享"
        ));
    }
}
