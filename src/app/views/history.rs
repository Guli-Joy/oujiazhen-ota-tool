//! 「历史下载」弹窗：记住每个包下到哪，重启后能接着续传
//!
//! 全量包动辄几个 G，一次下不完是常态。列表里的每一条都对应磁盘上的实况
//! （有 .partstate 就是可续传，只有文件就是已完成），点「继续下载」会沿用
//! 原来的路径与分块位图，不会从头再来，也不会重复占磁盘。
//!
//! 删除记录只删 JSON 里的一行，**不会碰用户的文件**，这一点在按钮文案上写明。

use crate::app::format::{bytes, date};
use crate::app::state::App;
use crate::core::history::{self, Status};
use crate::core::ota::Source;
use crate::core::theme::{self, P};
use eframe::egui;

/// 弹窗宽度
const W: f32 = 660.0;

/// 列表区域的最大高度：超过就滚动，弹窗本身不跟着长到屏幕外
const LIST_H: f32 = 372.0;

/// 行内三个按钮的最小宽度。
///
/// 中文字符宽度不同（「删除」2 字、「打开位置」4 字），如果统一给一个宽度，
/// 短按钮会留白、长按钮会撑出去盖住左边的文字，所以按各自字数分别定。
const BTN_DEL: f32 = 62.0;
const BTN_OPEN: f32 = 96.0;
const BTN_ACT: f32 = 86.0;

/// 右侧按钮组占的总宽度（含 8px 间距与 12px 呼吸位），左侧文字区要减掉它
const BTN_ZONE: f32 = BTN_DEL + BTN_OPEN + BTN_ACT + 8.0 * 2.0 + 12.0;

pub fn show(ctx: &egui::Context, app: &mut App) {
    if !app.history_open {
        return;
    }
    let mut open = true;
    egui::Window::new("历史下载")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_width(W)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .frame(
            egui::Frame::new()
                .fill(P::CARD)
                .stroke(egui::Stroke::new(1.0, P::BORDER_STRONG))
                .corner_radius(egui::CornerRadius::same(theme::CR_CARD))
                .inner_margin(egui::Margin::same(18)),
        )
        .show(ctx, |ui| {
            ui.set_max_width(W - 40.0);

            // 顶部：统计 + 重新扫描
            let entries: Vec<history::Entry> = app.history.entries().to_vec();
            let paused = entries
                .iter()
                .filter(|e| matches!(e.status, Status::Paused | Status::Downloading))
                .count();
            let done = entries.iter().filter(|e| e.status == Status::Done).count();

            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(match (paused, done, entries.len()) {
                        (0, 0, 0) => "还没有下载记录".to_string(),
                        _ => format!(
                            "{} 条记录 · 可续传 {} · 已完成 {}",
                            entries.len(),
                            paused,
                            done
                        ),
                    })
                    .size(12.5)
                    .color(P::TEXT_DIM),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::ghost_button(ui, "重新扫描")
                        .on_hover_text("按磁盘实况重新核对每条记录的状态与进度")
                        .clicked()
                    {
                        app.history.refresh();
                    }
                });
            });

            ui.add_space(10.0);

            if entries.is_empty() {
                empty(ui);
            } else {
                egui::ScrollArea::vertical()
                    .max_height(LIST_H)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for e in &entries {
                            ui.add_space(2.0);
                            row(ui, app, e);
                            ui.add_space(8.0);
                        }
                    });
            }

            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if theme::ghost_button_off(ui, "清空记录")
                    .on_hover_text("只删除下面的记录，磁盘上已下载的文件一个都不会动")
                    .clicked()
                {
                    app.history.clear();
                    app.set(crate::app::state::Tone::Info, "已清空历史下载记录");
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::primary_button(ui, "关闭").clicked() {
                        app.history_open = false;
                    }
                });
            });

            // 说明存档位置：用户想自己备份/清理时能找到
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(format!(
                    "记录保存在 {}（不放在下载目录里）",
                    history::store_path().display()
                ))
                .size(10.8)
                .color(P::TEXT_FAINT),
            );
        });
    if !open {
        app.history_open = false;
    }
}

/// 状态 → 强调色
fn status_color(s: Status) -> egui::Color32 {
    match s {
        Status::Paused => P::WARN,
        Status::Downloading => P::ACCENT,
        Status::Done => P::OK,
        Status::Failed => P::DANGER,
    }
}

fn row(ui: &mut egui::Ui, app: &mut App, e: &history::Entry) {
    let color = status_color(e.status);
    // 已完成的记录没有「继续/重试」可点，右侧就少一个按钮，左侧文字区可以放宽
    // 已经在下/已下完的任务没有「继续」可点
    let has_act = !matches!(e.status, Status::Done | Status::Downloading);
    let zone = BTN_ZONE - if has_act { 0.0 } else { BTN_ACT + 8.0 };

    theme::card_with(ui, theme::CardStyle::rail(color), |ui| {
        // 先把右侧按钮组的宽度占掉，左侧文字区按剩余空间收缩：
        // 这一步不做的话，窄窗口下长版本号会一路顶到按钮底下互相压字。
        let left_w = (ui.available_width() - zone).max(170.0);

        ui.horizontal(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(left_w, 0.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_max_width(left_w);

                    // 第一行：版本号 + 状态角标 + 来源 + 校验结果
                    ui.horizontal(|ui| {
                        let v = ui.add(
                            egui::Label::new(
                                egui::RichText::new(&e.version)
                                    .size(14.5)
                                    .strong()
                                    .color(P::TEXT),
                            )
                            .truncate(),
                        );
                        if !e.pkg.md5.is_empty() {
                            v.on_hover_text(format!("MD5\n{}", e.pkg.md5));
                        }
                        theme::soft_pill(ui, e.status.label(), color);
                        if e.pkg.source != Source::Official {
                            let c = if e.pkg.source == Source::AfterSale {
                                P::WARN
                            } else {
                                P::VIOLET
                            };
                            theme::soft_pill(ui, e.pkg.source.label(), c)
                                .on_hover_text(e.pkg.source.tip());
                        }
                        // 校验结果只在真的校验过时才有
                        match e.md5_ok {
                            Some(true) => {
                                theme::soft_pill(ui, "MD5 通过", P::OK);
                            }
                            Some(false) => {
                                theme::soft_pill(ui, "MD5 不符", P::DANGER).on_hover_text(
                                    "文件与官方哈希不一致，建议删除后重新下载",
                                );
                            }
                            None => {}
                        }
                    });

                    ui.add_space(6.0);

                    // 第二行：机型 · 代号 · 总大小 · 更新时间
                    ui.horizontal(|ui| {
                        let name = if e.model_name.is_empty() {
                            "未知机型"
                        } else {
                            e.model_name.as_str()
                        };
                        meta(ui, name);
                        dot(ui);
                        meta(ui, if e.model.is_empty() { "—" } else { e.model.as_str() });
                        dot(ui);
                        meta(ui, &bytes(if e.total > 0 { e.total } else { e.pkg.size }));
                        dot(ui);
                        meta(ui, &format!("更新 {}", date(e.updated * 1000)));
                    });
                },
            );

            // 右侧按钮从右往左排：删除 → 打开位置 →（可选的）继续/重试
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // 删除只影响记录：文案与 hover 都写清楚不碰文件
                if theme::ghost_button_min(ui, "删除", BTN_DEL)
                    .on_hover_text("只删除这条记录，不删除磁盘上已下载的文件")
                    .clicked()
                {
                    app.history.remove(&e.target);
                }
                ui.add_space(8.0);
                if theme::ghost_button_min(ui, "打开位置", BTN_OPEN)
                    .on_hover_text("在资源管理器中定位到文件（文件不在了就打开所在目录）")
                    .clicked()
                {
                    app.reveal_path(&e.target);
                }
                if has_act {
                    ui.add_space(8.0);
                    let (text, tip) = match e.status {
                        Status::Failed => ("重试", "从上次停下的分块继续"),
                        _ => ("继续下载", "沿用原来的路径与分块位图，不会从头开始"),
                    };
                    // Downloading 走不到这里（has_act 为 false），保留通用分支即可
                    if theme::primary_button_min(ui, text, BTN_ACT)
                        .on_hover_text(tip)
                        .clicked()
                    {
                        app.resume_from_history(e.clone());
                    }
                }
            });
        });

        // 进度条 + 明细
        let frac = if e.total > 0 {
            (e.bytes as f32 / e.total as f32).clamp(0.0, 1.0)
        } else {
            0.0
        };
        ui.add_space(10.0);
        theme::progress_bar_static(ui, frac, ui.available_width(), 6.0);
        ui.add_space(7.0);
        let pct = frac * 100.0;
        let line = match e.status {
            Status::Paused => format!(
                "{} / {} · {:.1}% · 缓存仍在，可继续",
                bytes(e.bytes),
                bytes(e.total),
                pct
            ),
            Status::Downloading => format!(
                "{} / {} · {:.1}% · 正在下载，暂停后可继续",
                bytes(e.bytes),
                bytes(e.total),
                pct
            ),
            Status::Done => format!("{} · 100% · 文件已就绪", bytes(e.total)),
            Status::Failed => format!(
                "{} / {} · {:.1}% · 上次出错停下，可重试",
                bytes(e.bytes),
                bytes(e.total),
                pct
            ),
        };
        ui.label(egui::RichText::new(line).size(11.2).color(P::TEXT_DIM));
        ui.add_space(4.0);
        // 路径单独一行、可截断：文件名通常很长，塞进上面那行会把关键数字挤掉
        ui.add(
            egui::Label::new(
                egui::RichText::new(&e.target)
                    .size(10.8)
                    .monospace()
                    .color(P::TEXT_FAINT),
            )
            .truncate(),
        )
        .on_hover_text(&e.target);
    });
}
fn empty(ui: &mut egui::Ui) {
    let h = 132.0;
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), h),
        egui::Sense::hover(),
    );
    let p = ui.painter();
    p.rect(
        rect,
        egui::CornerRadius::same(10),
        P::with_alpha(P::CARD, 120),
        egui::Stroke::new(1.0, P::BORDER),
        egui::StrokeKind::Inside,
    );
    let c = rect.center();
    // 空态图标：一个虚线框的「档案盒」
    let box_r = egui::Rect::from_center_size(c - egui::vec2(0.0, 14.0), egui::vec2(40.0, 30.0));
    let s = egui::Stroke::new(1.4, P::with_alpha(P::TEXT_FAINT, 200));
    p.rect_stroke(box_r, egui::CornerRadius::same(7), s, egui::StrokeKind::Inside);
    p.line_segment(
        [
            egui::pos2(box_r.left() + 8.0, box_r.center().y),
            egui::pos2(box_r.right() - 8.0, box_r.center().y),
        ],
        s,
    );
    p.text(
        c + egui::vec2(0.0, 20.0),
        egui::Align2::CENTER_CENTER,
        "还没有下载记录",
        egui::FontId::new(13.0, egui::FontFamily::Proportional),
        P::TEXT_DIM,
    );
    p.text(
        c + egui::vec2(0.0, 40.0),
        egui::Align2::CENTER_CENTER,
        "在版本列表里点「下载」后，这里会自动记下进度，下次打开还能接着下",
        egui::FontId::new(11.0, egui::FontFamily::Proportional),
        P::TEXT_FAINT,
    );
}

fn meta(ui: &mut egui::Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .size(11.2)
            .color(P::TEXT_DIM),
    );
}

fn dot(ui: &mut egui::Ui) {
    ui.label(
        egui::RichText::new("·")
            .size(11.2)
            .color(P::with_alpha(P::TEXT_FAINT, 150)),
    );
}