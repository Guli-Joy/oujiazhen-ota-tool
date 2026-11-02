//! 欧加真全量包下载工具 —— OPPO / realme / 一加 全量包查询与下载
//!
//! 结构：
//! - `core/` 与官方 OTA 服务器通信（协议、下载器、机型检索、主题）
//! - `app/`    界面状态与各个视图
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod core;
mod models;

use app::state::App;
use app::views;
use eframe::egui;

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll();

        // 后台有活动时保持刷新。
        // pausing() 也要算进来：暂停后排线程还要退一会儿，
        // 这期间不刷新的话，界面会一直停在"下载中"的旧画面上。
        if self.querying || self.resolving || self.is_downloading() || self.pausing() {
            ctx.request_repaint_after(std::time::Duration::from_millis(120));
        }

        let bare = egui::Frame::NONE;

        egui::Panel::top("topbar")
            .exact_size(views::topbar::HEIGHT)
            .frame(bare)
            .show(ui, |ui| views::topbar::show(ui, self));

        let bottom_h = views::bottom::height(&ctx, self);
        egui::Panel::bottom("bottombar")
            .exact_size(bottom_h)
            .frame(bare.fill(core::theme::P::BG_SIDE))
            .show(ui, |ui| views::bottom::show(ui, self));

        egui::Panel::left("sidebar")
            .exact_size(views::sidebar::WIDTH)
            .resizable(false)
            .show_separator_line(false)
            .frame(bare.fill(core::theme::P::BG_SIDE))
            .show(ui, |ui| {
                // 右侧一条分隔线
                let r = ui.max_rect();
                ui.painter().vline(
                    r.max.x - 0.5,
                    r.y_range(),
                    egui::Stroke::new(1.0, core::theme::P::BORDER),
                );
                views::sidebar::show(ui, self);
            });

        egui::CentralPanel::default()
            .frame(bare.fill(core::theme::P::BG))
            .show(ui, |ui| views::packages::show(ui, self));

        // 两个弹窗都盖在最上层：历史记录在前，数据来源在后
        views::history::show(&ctx, self);
        views::about::show(&ctx, self);

        // 剪贴板必须在所有视图画完之后再刷新：按钮是在上面这几行里被点中的，
        // 如果放在帧首，本帧刚设置的 pending_copy 要等下一帧才写进剪贴板，
        // 而点击之后 egui 未必还会再重画一次 —— 于是「路径已复制」只是嘴上说，
        // 剪贴板里其实什么都没有。放在帧尾，点中即复制。
        if let Some(text) = self.pending_copy.take() {
            ctx.copy_text(text);
        }
    }
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1180.0, 820.0])
            .with_min_inner_size([940.0, 660.0])
            .with_title(concat!(
                "欧加真全量包下载工具",
                " · OPPO / realme / 一加"
            )),
        ..Default::default()
    };
    // 支持 ota-tool.exe RMX3888 直接带机型启动；OTA_TOOL_DIR 可指定默认保存目录
    let initial = std::env::args().nth(1).filter(|s| !s.trim().is_empty());
    let dir = std::env::var("OTA_TOOL_DIR")
        .ok()
        .filter(|s| !s.trim().is_empty());

    eframe::run_native(
        "ota-tool",
        options,
        Box::new(move |cc| {
            core::theme::apply(&cc.egui_ctx);
            Ok(Box::new(App::new(initial.clone(), dir.clone())) as Box<dyn eframe::App>)
        }),
    )
}
