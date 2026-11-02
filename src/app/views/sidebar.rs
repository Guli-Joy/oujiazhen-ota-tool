//! 左侧：机型搜索 + 结果列表
use crate::app::state::App;
use crate::core::search::Hit;
use crate::core::theme::{self, P};
use eframe::egui;

/// 侧栏宽度
pub const WIDTH: f32 = 316.0;
/// 左右留白
const PAD_X: f32 = 14.0;
/// 单行高度
const ROW_H: f32 = 48.0;

pub fn show(ui: &mut egui::Ui, app: &mut App) {
    ui.add_space(16.0);

    // ---- 标题 ----
    ui.horizontal(|ui| {
        ui.add_space(PAD_X);
        ui.label(egui::RichText::new("机型搜索").size(12.0).strong().color(P::TEXT_FAINT));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(PAD_X);
            ui.label(
                egui::RichText::new(format!("{} 款", crate::models::MODELS.len()))
                    .size(10.8)
                    .color(P::with_alpha(P::TEXT_FAINT, 180)),
            );
        });
    });
    ui.add_space(9.0);

    // ---- 搜索框 ----
    search_box(ui, app);

    ui.add_space(9.0);

    // ---- 操作按钮 ----
    ui.horizontal(|ui| {
        ui.add_space(PAD_X);
        if theme::primary_button(ui, "搜索").clicked() {
            app.search();
        }
        ui.add_space(6.0);
        if theme::ghost_button(ui, "按代号查")
            .on_hover_text("把输入框内容当成机型代号直接查询，例如 PJZ110、PHK110")
            .clicked()
        {
            app.query_code();
        }
    });

    ui.add_space(13.0);
    divider(ui);

    // ---- 结果统计 ----
    if !app.results.is_empty() {
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.add_space(PAD_X);
            ui.label(
                egui::RichText::new(format!("匹配到 {} 个机型", app.results.len()))
                    .size(11.0)
                    .color(P::TEXT_FAINT),
            );
        });
    }
    ui.add_space(8.0);

    // ---- 结果列表 ----
    let h = (ui.available_height() - 8.0).max(60.0);
    let list_rect = egui::Rect::from_min_size(ui.cursor().min, egui::vec2(ui.available_width(), h));
    let mut pick: Option<usize> = None;
    let overflow = ui
        .scope_builder(egui::UiBuilder::new().max_rect(list_rect), |ui| {
            // 收一次 clip_rect，否则超出一屏的行会画到面板外面去
            ui.set_clip_rect(ui.clip_rect().intersect(list_rect));
            let out = egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if app.results.is_empty() {
                        placeholder(ui, app);
                        return;
                    }
                    for i in 0..app.results.len() {
                        let hit = app.results[i];
                        let sel = app.selected == Some(i);
                        ui.horizontal(|ui| {
                            ui.add_space(8.0);
                            if row(ui, &hit, sel).clicked() {
                                pick = Some(i);
                            }
                        });
                        ui.add_space(2.0);
                    }
                });
            out.content_size.y > list_rect.height() + 1.0
        })
        .inner;

    // 底部渐隐：暗示下面还有机型可滚，而不是让最后一行被硬生生切一半
    if overflow {
        let fade_h = 24.0_f32.min(list_rect.height() * 0.25);
        let fade = egui::Rect::from_min_max(
            egui::pos2(list_rect.left(), list_rect.bottom() - fade_h),
            list_rect.max,
        );
        theme::fade_bottom(ui.painter(), fade, P::BG_SIDE);
    }

    if let Some(i) = pick {
        app.selected = Some(i);
        app.query_selected();
    }
}

/// 圆角输入框 + 清空按钮；聚焦时外框换成强调色光晕。
fn search_box(ui: &mut egui::Ui, app: &mut App) {
    ui.horizontal(|ui| {
        ui.add_space(PAD_X);
        let w = WIDTH - PAD_X * 2.0;

        // 外框底色与边线由本函数自绘，才能做聚焦光晕的平滑过渡
        let painter = ui.painter().clone();
        let slot = painter.add(egui::Shape::Noop);

        let mut has_focus = false;
        let mut focus_id = None;
        let out = egui::Frame::new()
            .fill(egui::Color32::TRANSPARENT)
            .stroke(egui::Stroke::NONE)
            .corner_radius(egui::CornerRadius::same(9))
            .inner_margin(egui::Margin::symmetric(11, 0))
            .show(ui, |ui| {
                ui.set_width(w - 22.0);
                ui.set_height(36.0);
                ui.horizontal_centered(|ui| {
                    // 自绘放大镜（不依赖字体里的符号字形，任何机器都一致）
                    let tone = if app.keyword.is_empty() {
                        P::TEXT_FAINT
                    } else {
                        P::with_alpha(P::ACCENT, 210)
                    };
                    magnifier(ui, tone);
                    ui.add_space(5.0);
                    let btn_w = if app.keyword.is_empty() { 0.0 } else { 22.0 };
                    let tw = (ui.available_width() - btn_w - 4.0).max(60.0);
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut app.keyword)
                            .hint_text("GT5 Pro / Find X8 / RMX3888")
                            .desired_width(tw)
                            .frame(egui::Frame::NONE)
                            .text_color(P::TEXT)
                            .margin(egui::Margin::ZERO),
                    );
                    has_focus = resp.has_focus();
                    focus_id = Some(resp.id);
                    if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        if !app.keyword.trim().is_empty() {
                            app.search();
                        }
                    }
                    if !app.keyword.is_empty() && clear_btn(ui).clicked() {
                        app.keyword.clear();
                    }
                });
            });

        let rect = out.response.rect;
        let t = ui
            .ctx()
            .animate_bool(focus_id.unwrap_or(out.response.id), has_focus);
        let cr = egui::CornerRadius::same(9);
        painter.set(slot, egui::Shape::rect_filled(rect, cr, lerp(P::CARD, P::BG_SIDE, t * 0.5)));
        painter.rect_stroke(
            rect,
            cr,
            egui::Stroke::new(1.0, lerp(P::BORDER, P::ACCENT, t * 0.85)),
            egui::StrokeKind::Inside,
        );
        if t > 0.01 {
            // 一圈极淡的强调色光晕
            painter.rect_stroke(
                rect.expand(2.5),
                egui::CornerRadius::same(11),
                egui::Stroke::new(1.0, P::with_alpha(P::ACCENT, (t * 42.0) as u8)),
                egui::StrokeKind::Outside,
            );
        }
    });
}

/// 两个颜色之间线性插值（t = 0 取 a，t = 1 取 b）。
fn lerp(a: egui::Color32, b: egui::Color32, t: f32) -> egui::Color32 {
    let t = t.clamp(0.0, 1.0);
    let f = |x: u8, y: u8| -> u8 { (x as f32 + (y as f32 - x as f32) * t).round() as u8 };
    let [ar, ag, ab, aa] = a.to_array();
    let [br, bg, bb, ba] = b.to_array();
    egui::Color32::from_rgba_premultiplied(f(ar, br), f(ag, bg), f(ab, bb), f(aa, ba))
}

/// 放大镜图标：圆圈 + 45° 手柄，尺寸 14x14。
fn magnifier(ui: &mut egui::Ui, color: egui::Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
    let p = ui.painter();
    let s = egui::Stroke::new(1.5, color);
    let c = rect.center() - egui::vec2(1.0, 1.0);
    p.circle_stroke(c, 4.2, s);
    p.line_segment(
        [c + egui::vec2(3.2, 3.2), c + egui::vec2(6.4, 6.4)],
        s,
    );
}

fn clear_btn(ui: &mut egui::Ui) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::click());
    let c = if resp.hovered() { P::TEXT } else { P::TEXT_FAINT };
    let p = ui.painter();
    let r = egui::Rect::from_center_size(rect.center(), egui::vec2(7.0, 7.0));
    let s = egui::Stroke::new(1.4, c);
    p.line_segment([r.left_top(), r.right_bottom()], s);
    p.line_segment([r.right_top(), r.left_bottom()], s);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

fn placeholder(ui: &mut egui::Ui, _app: &App) {
    ui.add_space(34.0);

    // 小图标：虚线圆 + 中心点，暗示「这里是搜索结果的占位」
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, 46.0), egui::Sense::hover());
    let p = ui.painter();
    let c = rect.center();
    p.circle_stroke(
        c,
        18.0,
        egui::Stroke::new(1.0, P::with_alpha(P::TEXT_FAINT, 90)),
    );
    p.circle_filled(c, 3.0, P::with_alpha(P::TEXT_FAINT, 140));

    ui.add_space(8.0);
    ui.vertical_centered(|ui| {
        ui.label(egui::RichText::new("还没有搜索结果").size(12.5).color(P::TEXT_DIM));
        ui.add_space(5.0);
        ui.label(
            egui::RichText::new("输入机型名称或代号后回车")
                .size(11.0)
                .color(P::TEXT_FAINT),
        );
    });
}

fn row(ui: &mut egui::Ui, hit: &Hit, selected: bool) -> egui::Response {
    let w = ui.available_width() - 6.0;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, ROW_H), egui::Sense::click());
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let p = ui.painter();
    if selected {
        p.rect_filled(rect, 9, P::accent_soft(30));
        p.rect_stroke(
            rect,
            9,
            egui::Stroke::new(1.0, P::accent_soft(130)),
            egui::StrokeKind::Inside,
        );
        // 左侧强调竖条
        p.rect_filled(
            egui::Rect::from_min_size(
                egui::pos2(rect.left() + 2.5, rect.top() + 10.0),
                egui::vec2(2.5, rect.height() - 20.0),
            ),
            2,
            P::ACCENT,
        );
    } else if resp.hovered() {
        p.rect_filled(rect, 9, P::CARD_HOVER);
    }

    // 品牌色圆形头像：内部放品牌首字，给列表一个视觉锚点
    let accent = theme::brand_color(hit.brand);
    let av = egui::Rect::from_center_size(
        egui::pos2(rect.left() + 26.0, rect.center().y),
        egui::vec2(26.0, 26.0),
    );
    let av_fill = if selected {
        P::with_alpha(accent, 64)
    } else {
        P::with_alpha(accent, 30)
    };
    p.circle_filled(av.center(), 13.0, av_fill);
    p.text(
        av.center(),
        egui::Align2::CENTER_CENTER,
        theme::brand_glyph(hit.brand),
        egui::FontId::new(11.5, egui::FontFamily::Proportional),
        accent,
    );

    let tx = rect.left() + 46.0;
    p.text(
        egui::pos2(tx, rect.center().y - 8.0),
        egui::Align2::LEFT_CENTER,
        hit.name,
        egui::FontId::new(13.2, egui::FontFamily::Proportional),
        P::TEXT,
    );
    p.text(
        egui::pos2(tx, rect.center().y + 10.0),
        egui::Align2::LEFT_CENTER,
        format!("{}  ·  {}", hit.code, hit.brand),
        egui::FontId::new(10.8, egui::FontFamily::Monospace),
        if selected { P::ACCENT } else { P::TEXT_FAINT },
    );
    resp
}



fn divider(ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.add_space(PAD_X);
        let w = WIDTH - PAD_X * 2.0;
        let (rect, _) = ui.allocate_exact_size(egui::vec2(w, 1.0), egui::Sense::hover());
        ui.painter()
            .hline(rect.x_range(), rect.center().y, egui::Stroke::new(1.0, P::BORDER));
    });
}
