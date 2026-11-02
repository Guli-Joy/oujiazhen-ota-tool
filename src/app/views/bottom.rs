//! 底部：保存位置 / 线程 / 下载进度 / 状态栏
use crate::app::format;
use crate::app::state::{App, Tone};
use crate::core::theme::{self, P};
use eframe::egui;

/// 上下内边距
const PAD_V: f32 = 13.0;
/// 第一行（保存位置）高度
const ROW1_H: f32 = 34.0;
/// 进度块高度（标题行 30 + 间隔 9 + 进度条 8 + 间隔 9 + 明细行 18 ≈ 74，留点余量）
const PROG_H: f32 = 78.0;
/// 状态行高度
const STATUS_H: f32 = 20.0;

/// 上一帧实测内容高度存在这里的 key
fn measured_id() -> egui::Id {
    egui::Id::unique("bottom_content_h")
}

/// 底部面板该有多高（随是否在下载变化）。
///
/// 常量只是「估算值」：字体、按钮内边距、明细行有没有折行，都会让真实高度
/// 差十几个像素。差额一旦为负，最下面的状态行就被面板裁掉（进度条下方那行
/// 「进度已保存…」看着像没了）。所以上一帧画完时把实测高度记下来，
/// 这一帧取两者较大值，一帧内自愈，不必再去手调常量。
pub fn height(ctx: &egui::Context, app: &App) -> f32 {
    let mut h = PAD_V * 2.0 + ROW1_H + 12.0 + STATUS_H;
    if app.download.is_some() {
        h += PROG_H + 12.0;
        if app
            .progress_snapshot()
            .map(|p| p.error.is_some())
            .unwrap_or(false)
        {
            h += 16.0;
        }
        if let Some(m) = ctx.data(|d| d.get_temp::<f32>(measured_id())) {
            // m 是内容本身的高度，面板还要加上下 PAD_V 的内边距，
            // 之前漏了这一项，于是面板永远比内容矮 26px，最底下那行被裁掉。
            h = h.max(m + PAD_V * 2.0 + 1.0);
        }
    }
    h
}

pub fn show(ui: &mut egui::Ui, app: &mut App) {
    let rect = ui.max_rect();
    let p = ui.painter();
    p.rect_filled(rect, 0, P::BG_SIDE);
    p.hline(
        rect.x_range(),
        rect.min.y + 0.5,
        egui::Stroke::new(1.0, P::BORDER),
    );

    let inner = rect.shrink2(egui::vec2(24.0, PAD_V));
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(inner)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    let ui = &mut child;

    row_dir(ui, app);
    if app.download.is_some() {
        ui.add_space(10.0);
        row_progress(ui, app);
    }
    ui.add_space(8.0);
    row_status(ui, app);

    // 记下这一帧真实用掉的高度，供下一帧的 height() 校正（见 height 的注释）。
    // min_rect 取的是本帧所有已分配区域的并集，内容超出面板 max_rect 时它照样变大，
    // 所以能如实反映「内容需要多高」。
    if app.download.is_some() {
        let used = ui.min_rect().max.y - inner.min.y;
        ui.ctx().data_mut(|d| d.insert_temp(measured_id(), used));
    }
}

fn row_dir(ui: &mut egui::Ui, app: &mut App) {
    let right_group = 250.0;
    let path_w = (ui.available_width() - right_group).max(200.0);

    ui.horizontal(|ui| {
        // ---- 左：标签 + 路径框 + 浏览 ----
        ui.allocate_ui_with_layout(
            egui::vec2(path_w, ROW1_H),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.label(egui::RichText::new("保存到").size(11.5).color(P::TEXT_FAINT));
                ui.add_space(9.0);
                let box_w = (ui.available_width() - 76.0).max(120.0);
                egui::Frame::new()
                    .fill(P::CARD)
                    .stroke(egui::Stroke::new(1.0, P::BORDER))
                    .corner_radius(egui::CornerRadius::same(8))
                    .inner_margin(egui::Margin::symmetric(10, 0))
                    .show(ui, |ui| {
                        ui.set_width(box_w);
                        ui.set_height(ROW1_H - 6.0);
                        ui.horizontal_centered(|ui| {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(&app.save_dir)
                                        .size(11.3)
                                        .monospace()
                                        .color(P::TEXT_DIM),
                                )
                                .truncate(),
                            );
                        });
                    });
                ui.add_space(8.0);
                if theme::ghost_button(ui, "浏览").clicked() {
                    if let Some(dir) = rfd::FileDialog::new()
                        .set_directory(&app.save_dir)
                        .pick_folder()
                    {
                        app.save_dir = dir.display().to_string();
                        app.set(Tone::Info, format!("保存目录已改为 {}", app.save_dir));
                    }
                }
            },
        );

        // ---- 右：线程数（右对齐，自绘步进器） ----
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if let Some(v) = stepper(ui, app.threads as u32, 1, 64) {
                app.threads = v as usize;
            }
            ui.add_space(8.0);
            ui.label(egui::RichText::new("线程").size(11.5).color(P::TEXT_FAINT));
        });
    });
}

/// 自绘步进器：[−] 数值 [+]；两端到顶时按钮自动变暗。
/// 返回 Some(新值) 表示本帧被修改。
fn stepper(ui: &mut egui::Ui, value: u32, min: u32, max: u32) -> Option<u32> {
    const H: f32 = 26.0;
    const BTN: f32 = 22.0;
    const VAL: f32 = 34.0;
    let w = BTN * 2.0 + VAL;

    let (rect, _resp) = ui.allocate_exact_size(egui::vec2(w, H), egui::Sense::hover());
    let cr = egui::CornerRadius::same(7);
    let p = ui.painter();
    p.rect(
        rect,
        cr,
        P::CARD,
        egui::Stroke::new(1.0, P::BORDER),
        egui::StrokeKind::Inside,
    );

    let mut out = None;
    // 左减号
    let dec = egui::Rect::from_min_size(rect.min, egui::vec2(BTN, H));
    let dec_hot = ui
        .interact(dec, ui.scope_id().with("stepper_dec"), egui::Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    // 右加号
    let inc = egui::Rect::from_min_size(
        egui::pos2(rect.max.x - BTN, rect.top()),
        egui::vec2(BTN, H),
    );
    let inc_hot = ui
        .interact(inc, ui.scope_id().with("stepper_inc"), egui::Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);

    if dec_hot.clicked() && value > min {
        out = Some(value - 1);
    }
    if inc_hot.clicked() && value < max {
        out = Some(value + 1);
    }

    let dec_on = value > min;
    let inc_on = value < max;
    let dec_c = if !dec_on {
        P::with_alpha(P::TEXT_FAINT, 110)
    } else if dec_hot.hovered() {
        P::TEXT
    } else {
        P::TEXT_DIM
    };
    let inc_c = if !inc_on {
        P::with_alpha(P::TEXT_FAINT, 110)
    } else if inc_hot.hovered() {
        P::TEXT
    } else {
        P::TEXT_DIM
    };

    // hover 底色
    if dec_hot.hovered() && dec_on {
        p.rect_filled(dec.shrink(3.0), egui::CornerRadius::same(5), P::CARD_HOVER);
    }
    if inc_hot.hovered() && inc_on {
        p.rect_filled(inc.shrink(3.0), egui::CornerRadius::same(5), P::CARD_HOVER);
    }

    // 竖分隔线
    for x in [rect.left() + BTN, rect.right() - BTN] {
        p.line_segment(
            [egui::pos2(x, rect.top() + 5.0), egui::pos2(x, rect.bottom() - 5.0)],
            egui::Stroke::new(1.0, P::BORDER),
        );
    }

    // 减号
    let s = egui::Stroke::new(1.5, dec_c);
    let c = dec.center();
    p.line_segment([egui::pos2(c.x - 4.0, c.y), egui::pos2(c.x + 4.0, c.y)], s);
    // 加号
    let c = inc.center();
    let s = egui::Stroke::new(1.5, inc_c);
    p.line_segment([egui::pos2(c.x - 4.0, c.y), egui::pos2(c.x + 4.0, c.y)], s);
    p.line_segment([egui::pos2(c.x, c.y - 4.0), egui::pos2(c.x, c.y + 4.0)], s);

    // 数值
    p.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        value.to_string(),
        egui::FontId::new(12.5, egui::FontFamily::Proportional),
        P::TEXT,
    );

    out
}

fn row_progress(ui: &mut egui::Ui, app: &mut App) {
    let Some(pr) = app.progress_snapshot() else {
        return;
    };
    let name = app
        .download
        .as_ref()
        .map(|d| d.name.clone())
        .unwrap_or_default();
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(&name).size(12.0).color(P::TEXT));
        let resumable = app.can_resume();
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // 右侧按钮随状态切换：
            // 下载中 →「暂停」；暂停 →「继续下载」；出错 →「重试」；完成 →「打开文件夹」＋「关闭」。
            // 只要磁盘上有那个文件，「打开文件夹」就可用。
            if pr.active {
                // 点了暂停之后，下载线程还要等当前分块读完才退干净（网络卡顿时最多几秒）。
                // 这段时间按钮必须先变成「暂停中…」，否则用户以为没点上，会连点好几次。
                if app.pausing() {
                    theme::busy_button_min(ui, "暂停中…", 0.0)
                        .on_hover_text("正在等待分块读完，进度随即落盘");
                } else if theme::danger_button(ui, "暂停")
                    .on_hover_text("立即暂停，已完成的分块会保存，随时可继续")
                    .clicked()
                {
                    app.stop_download();
                    // 点下去这一帧就要重画：按钮必须立刻变成「暂停中…」，
                    // 靠 120ms 的定时重绘会让人以为没点上，于是连点好几次。
                    ui.ctx().request_repaint();
                }
            } else if pr.done {
                if theme::ghost_button(ui, "关闭").clicked() {
                    app.clear_finished();
                }
                ui.add_space(8.0);
                if theme::ghost_button(ui, "打开文件夹").clicked() {
                    app.reveal_download();
                }
            } else if resumable {
                if theme::primary_button(ui, "继续下载")
                    .on_hover_text("从已下载的分块接着下，不会从头开始")
                    .clicked()
                {
                    app.resume_download();
                }
                ui.add_space(8.0);
                if theme::ghost_button(ui, "打开文件夹").clicked() {
                    app.reveal_download();
                }
            } else {
                // 出错了：还能接着试，也允许定位到已下到一半的文件
                if theme::primary_button(ui, "重试").clicked() {
                    app.resume_download();
                }
                ui.add_space(8.0);
                if theme::ghost_button(ui, "打开文件夹").clicked() {
                    app.reveal_download();
                }
            }
        });
    });

    ui.add_space(9.0);
    let frac = (pr.percent() / 100.0).clamp(0.0, 1.0);
    if pr.active {
        theme::progress_bar(ui, frac, ui.available_width(), 8.0);
    } else {
        theme::progress_bar_static(ui, frac, ui.available_width(), 8.0);
    }

    ui.add_space(9.0);
    let (left, color) = if pr.verifying {
        ("正在校验 MD5…".to_string(), P::TEXT_DIM)
    } else if pr.done {
        match pr.verified {
            Some(true) => ("下载完成 · MD5 校验通过".to_string(), P::OK),
            Some(false) => ("下载完成 · MD5 不匹配".to_string(), P::DANGER),
            None => ("下载完成".to_string(), P::OK),
        }
    } else if pr.active {
        (
            format!(
                "{} / {}   ·   {}   ·   剩余 {}   ·   {:.1}%",
                format::bytes(pr.downloaded),
                format::bytes(pr.total),
                pr.speed_text(),
                pr.eta_text(),
                pr.percent()
            ),
            P::TEXT_DIM,
        )
    } else if pr.paused {
        (
            format!(
                "已暂停 · {} / {}（{:.1}%）· 进度已保存，可继续下载",
                format::bytes(pr.downloaded),
                format::bytes(pr.total),
                pr.percent()
            ),
            P::WARN,
        )
    } else if pr.error.is_some() {
        (
            format!(
                "已停止 · {} / {}（{:.1}%）· 进度已保存",
                format::bytes(pr.downloaded),
                format::bytes(pr.total),
                pr.percent()
            ),
            P::DANGER,
        )
    } else {
        (
            format!(
                "已暂停 · {} / {}（{:.1}%）",
                format::bytes(pr.downloaded),
                format::bytes(pr.total),
                pr.percent()
            ),
            P::WARN,
        )
    };

    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(left).size(11.3).color(color));
        // 出错原因单独放右侧，不和进度数字挤在一行
        if let Some(err) = &pr.error {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(err).size(11.0).color(P::DANGER),
                    )
                    .truncate(),
                );
            });
        }
    });
}

fn row_status(ui: &mut egui::Ui, app: &App) {
    ui.horizontal(|ui| {
        let c = match app.tone {
            Tone::Ok => P::OK,
            Tone::Warn => P::WARN,
            Tone::Err => P::DANGER,
            Tone::Info => P::ACCENT,
        };
        let (rect, _) = ui.allocate_exact_size(egui::vec2(9.0, STATUS_H), egui::Sense::hover());
        ui.painter()
            .circle_filled(rect.center(), 3.0, P::with_alpha(c, 210));
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(&app.status)
                .size(11.5)
                .color(if app.tone == Tone::Err {
                    P::DANGER
                } else {
                    P::TEXT_DIM
                }),
        );

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if let Some(left) = app.link_left() {
                let c = if left < 90 { P::WARN } else { P::TEXT_FAINT };
                ui.label(
                    egui::RichText::new(format!("直链剩余 {}", format::dur(left)))
                        .size(11.0)
                        .color(c),
                );
                ui.add_space(12.0);
            }
            ui.label(
                egui::RichText::new("官方 OTA + VioletToolBox（Smart-Paocai, GPL-3.0）")
                    .size(10.8)
                    .color(P::TEXT_FAINT),
            )
            .on_hover_text(
                "版本清单：OPPO 官方 OTA 服务器 + VioletToolBox 社区库\n\
                 VioletToolBox 作者 Smart-Paocai，GNU GPL v3.0\n\
                 https://github.com/Smart-Paocai/VioletToolBox\n\
                 本工具仅调用其公开接口，未复制其源代码",
            );
        });
    });
}
