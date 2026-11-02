//! 顶部标题栏
use crate::app::state::App;
use crate::core::theme::{self, P};
use eframe::egui;

pub const HEIGHT: f32 = 68.0;

/// 程序名。窗口标题、关于框、README 都用同一个名字，改这里就够了。
pub const TITLE: &str = "欧加真全量包下载工具";

pub fn show(ui: &mut egui::Ui, app: &mut App) {
    let full = egui::Rect::from_min_size(
        ui.min_rect().min,
        egui::vec2(ui.available_width(), HEIGHT),
    );
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), HEIGHT), egui::Sense::hover());
    // 克隆一份画笔：下面还要 &mut ui 去画右侧入口，不能一直借着它
    let p = ui.painter().clone();

    // 背景 + 底部细线
    p.rect_filled(rect, 0, P::BG);
    p.hline(rect.x_range(), rect.max.y - 0.5, egui::Stroke::new(1.0, P::BORDER));


    // 左侧：logo 方块（渐变）
    let logo = egui::Rect::from_min_size(rect.min + egui::vec2(22.0, 16.0), egui::vec2(36.0, 36.0));
    theme::gradient_rect(&p, logo, P::ACCENT, P::VIOLET, 11);
    let c = logo.center();
    let g = egui::Stroke::new(1.9, egui::Color32::WHITE);
    p.line_segment([egui::pos2(c.x, c.y - 8.0), egui::pos2(c.x, c.y + 3.0)], g);
    p.line_segment([egui::pos2(c.x - 5.0, c.y - 2.5), egui::pos2(c.x, c.y + 3.5)], g);
    p.line_segment([egui::pos2(c.x + 5.0, c.y - 2.5), egui::pos2(c.x, c.y + 3.5)], g);
    p.line_segment([egui::pos2(c.x - 7.0, c.y + 8.0), egui::pos2(c.x + 7.0, c.y + 8.0)], g);

    // 标题
    let tx = logo.max.x + 13.0;
    p.text(
        egui::pos2(tx, rect.min.y + 27.0),
        egui::Align2::LEFT_CENTER,
        TITLE,
        egui::FontId::new(17.5, egui::FontFamily::Proportional),
        P::TEXT,
    );
    p.text(
        egui::pos2(tx, rect.min.y + 46.0),
        egui::Align2::LEFT_CENTER,
        "OPPO · realme · 一加   官方 OTA · 社区版本库 · 售后全量包",
        egui::FontId::new(11.5, egui::FontFamily::Proportional),
        P::TEXT_FAINT,
    );

    // 右侧两个文字入口。「数据来源」必须一眼能找到（用了别人的数据就得能一路点到出处），
    // 「历史下载」是高频入口，排在它左边。两者共用 entry() 排版，宽度不写死。
    let cy = rect.center().y;
    let right = rect.max.x - 22.0;
    let (left_about, about_resp) = entry(
        ui,
        right,
        cy,
        "数据来源 · 致谢",
        "about_entry",
        "版本清单来自三处：\n\
         ① OPPO 官方 OTA 服务器\n\
         ② VioletToolBox（作者 Smart-Paocai，GNU GPL v3.0）\n\
         ③ OPPO 售后降级服务器\n\
         点击查看完整来源说明",
    );
    // 数据源可信的小绿点，跟在「数据来源」左边
    let dot = egui::pos2(left_about - 12.0, cy);
    p.circle_filled(dot, 6.0, P::with_alpha(P::OK, 40));
    p.circle_filled(dot, 3.2, P::OK);
    if about_resp.clicked() {
        app.about_open = true;
    }

    let (_, hist_resp) = entry(
        ui,
        left_about - 28.0,
        cy,
        "历史下载",
        "history_entry",
        "查看下载过 / 下到一半的包，缓存还在就能接着下",
    );
    if hist_resp.clicked() {
        app.history_open = true;
    }

    // 顶部一道细渐变强调线
    theme::gradient_rect(
        &p,
        egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), 2.0)),
        P::ACCENT,
        P::VIOLET,
        0,
    );
    let _ = full;
}

/// 右上角的一个文字入口：右边界对齐到 `right`，返回它的左边界与交互结果。
///
/// 返回左边界是为了让下一个入口接着往左排，不用把每个标签的宽度算死在常量里。
fn entry(
    ui: &mut egui::Ui,
    right: f32,
    cy: f32,
    label: &str,
    id: &str,
    tip: &str,
) -> (f32, egui::Response) {
    let p = ui.painter().clone();
    let font = egui::FontId::new(11.5, egui::FontFamily::Proportional);
    let tw = p
        .layout_no_wrap(label.to_string(), font.clone(), P::TEXT_DIM)
        .size()
        .x;
    let hit = egui::Rect::from_min_max(
        egui::pos2(right - tw - 10.0, cy - 12.0),
        egui::pos2(right, cy + 12.0),
    );
    let resp = ui
        .interact(hit, ui.scope_id().with(id), egui::Sense::click())
        .on_hover_text(tip);
    let tint = if resp.hovered() { P::TEXT } else { P::TEXT_DIM };
    let gal = p.layout_no_wrap(label.to_string(), font, tint);
    p.galley(egui::pos2(right - tw, cy - gal.size().y / 2.0), gal, tint);
    if resp.hovered() {
        p.hline(
            (right - tw)..=right,
            cy + 9.0,
            egui::Stroke::new(1.0, P::with_alpha(P::TEXT_DIM, 160)),
        );
    }
    (right - tw, resp)
}
