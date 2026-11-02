//! 「数据来源」弹窗：把第三方来源、作者、许可证写清楚
//!
//! 只要界面里出现了别人的数据，就必须能一路点到这条说明，
//! 避免让人误以为这些版本清单是本工具自己收集的。

use crate::app::state::App;
use crate::core::theme::{self, P};
use crate::core::violet;
use eframe::egui;

/// 弹窗宽度
const W: f32 = 560.0;

pub fn show(ctx: &egui::Context, app: &mut App) {
    if !app.about_open {
        return;
    }
    let mut open = true;
    egui::Window::new("数据来源与致谢")
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

            block(
                ui,
                "官方源",
                P::ACCENT,
                &[
                    ("来源", "OPPO / realme / 一加 官方 OTA 服务器"),
                    ("地址", "component-ota-cn.allawntech.com"),
                    ("内容", "各分支当前最新版本、文件大小、安全补丁、MD5"),
                ],
            );

            ui.add_space(14.0);

            block(
                ui,
                "社区源（第三方）",
                P::VIOLET,
                &[
                    ("项目", "VioletToolBox"),
                    ("作者", violet::AUTHOR),
                    ("许可证", violet::LICENSE),
                    ("代码仓库", violet::REPO),
                    ("数据服务", violet::SITE),
                ],
            );

            ui.add_space(14.0);

            block(
                ui,
                "售后源（官方）",
                P::WARN,
                &[
                    ("来源", "OPPO 售后降级服务器"),
                    ("地址", "downgrade.coloros.com"),
                    ("内容", "维修站 / 降级用完整固件（售后全量包）"),
                ],
            );

            ui.add_space(12.0);
            ui.label(
                egui::RichText::new(
                    "本工具只调用 VioletToolBox 公开的 HTTP 接口来查询历史版本清单与下载地址，\
                     未复制、未修改、未分发其源代码。社区源收录的历史版本由其作者维护，\
                     本工具不对其准确性负责；官方源始终是主路径，社区源不可用时自动退回官方源。",
                )
                .size(11.8)
                .color(P::TEXT_DIM),
            );

            ui.add_space(10.0);
            ui.label(
                egui::RichText::new(
                    "社区源的部分旧版本地址由第三方服务器签发，可能已失效；\
                     卡片上标「链接已失效」的版本无法下载。",
                )
                .size(11.2)
                .color(P::TEXT_FAINT),
            );

            ui.add_space(8.0);
            ui.label(
                egui::RichText::new(
                    "售后全量包与升级包不在同一套版本序列，因此单独成区、列在列表最前面。\
                     官方的那批由 OPPO 售后服务器按项目号下发，地址是长期有效直链，续传不依赖签名刷新；\
                     社区源收录的那批（角标「社区售后」）给官方没下发降级包的机型兜底，\
                     它来自 VioletToolBox 的售后通道，一个机型常有三十多个版本，\
                     分「AB 全量包」（可在系统更新里本地升级）与「线刷包 OFP」（要用 EDL 工具走 9008 刷入）两类。\
                     降级会清空数据，刷入前请自行备份。",
                )
                .size(11.2)
                .color(P::TEXT_FAINT),
            );

            ui.add_space(16.0);
            ui.horizontal(|ui| {
                if theme::primary_button(ui, "知道了").clicked() {
                    app.about_open = false;
                }
                ui.add_space(8.0);
                if theme::ghost_button(ui, "打开项目主页").clicked() {
                    let _ = open::that(violet::REPO);
                }
            });
        });
    if !open {
        app.about_open = false;
    }
}

fn block(ui: &mut egui::Ui, title: &str, color: egui::Color32, rows: &[(&str, &str)]) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(4.0, 14.0), egui::Sense::hover());
        ui.painter()
            .rect_filled(rect, egui::CornerRadius::same(2), color);
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new(title)
                .size(13.0)
                .strong()
                .color(P::TEXT),
        );
    });
    ui.add_space(6.0);
    for (k, v) in rows {
        ui.horizontal(|ui| {
            ui.add_space(10.0);
            ui.label(
                egui::RichText::new(*k)
                    .size(11.5)
                    .color(P::TEXT_FAINT),
            );
            ui.add_space(10.0);
            ui.label(
                egui::RichText::new(*v)
                    .size(11.8)
                    .monospace()
                    .color(P::TEXT_DIM),
            );
        });
        ui.add_space(2.0);
    }
}
