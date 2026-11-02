//! 右侧：可用全量包列表
use crate::app::format::{bytes, date};
use crate::app::state::App;
use crate::core::ota::{AfterState, Package, Source};
use crate::core::theme::{self, P};
use eframe::egui;

/// 左右留白
const PAD_X: f32 = 24.0;
/// 底部脚注占用的高度
const FOOTER_H: f32 = 34.0;
/// 卡片内按钮的固定宽度（切换文字时不抖动）
const BTN_W: f32 = 74.0;
const BTN_W2: f32 = 74.0;

/// 用户在这一帧里点的动作，循环结束后统一执行，避免借用冲突。
///
/// 直接带上 Package 而不是下标：列表现在分「版本清单」和「售后包」两段，
/// 下标要额外记一个「属于哪一段」，反而更容易错。
enum Act {
    Download(Package),
    Resume,
    ResolveLink(Package),
    /// 展开 / 收起售后全量包。
    ///
    /// 售后通道一个机型能给三十多个包，全铺开会把下面「可用全量包」推出屏幕，
    /// 所以默认只露前几张，其余折起来等用户点开。
    ToggleAfter,
}

pub fn show(ui: &mut egui::Ui, app: &mut App) {
    header(ui, app);

    if app.querying {
        loading(ui);
        footer(ui);
        return;
    }
    if app.packages.is_empty() && app.aftersale.is_empty() {
        empty(ui, app);
        footer(ui);
        return;
    }

    let mut acts: Vec<Act> = Vec::new();

    let official = app
        .packages
        .iter()
        .filter(|p| p.source == Source::Official)
        .count();
    // 列表与脚注用显式矩形分区：
    // 底栏会随下载状态展开/收起，可用高度每帧都可能变，靠 max_height 容易让内容压到脚注上。
    let avail = ui.available_rect_before_wrap();
    let list_bottom = (avail.max.y - FOOTER_H).max(avail.min.y + 120.0);
    let list_rect =
        egui::Rect::from_min_max(avail.min, egui::pos2(avail.max.x, list_bottom));

    let overflow = ui
        .scope_builder(egui::UiBuilder::new().max_rect(list_rect), |ui| {
            // 裁剪区只在这里设一次：max_rect 只约束排版，不裁已绘制的图形，
            // 不显式裁剪卡片就会漏到脚注底下。注意不能在滚动内容里再设一次，
            // 那会覆盖 ScrollArea 自己按滚动偏移算好的裁剪区，滚动后内容会错位。
            ui.set_clip_rect(ui.clip_rect().intersect(list_rect));
            let out = egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    // 售后全量包放在最前：它通常只有 1~2 个，而升级包动辄二十几个，
                    // 排在二十多张卡后面等于藏起来了（1180x820 下一屏只放得下 3 张）。
                    if !app.aftersale.is_empty() {
                        let shown = app.after_visible();
                        if let Some(a) =
                            after_label(ui, app.aftersale.len(), app.after_open, shown.len())
                        {
                            acts.push(a);
                        }
                        for pkg in app.after_visible().iter() {
                            if let Some(a) = card(ui, app, pkg, false) {
                                acts.push(a);
                            }
                            ui.add_space(10.0);
                        }
                        ui.add_space(8.0);
                    } else if app.after_state == AfterState::Unknown {
                        // 没查到和没有是两回事：前者值得再点一次「重新扫描」，
                        // 静默留白只会让人以为售后功能坏了。
                        after_pending(ui);
                    }
                    if !app.packages.is_empty() {
                        section_label(ui, app.packages.len(), official);
                    }
                    for (i, pkg) in app.packages.iter().enumerate() {
                        // 首位即最新版本，给它更强的主次层次
                        let latest = i == 0;
                        if let Some(a) = card(ui, app, pkg, latest) {
                            acts.push(a);
                        }
                        ui.add_space(10.0);
                    }
                    ui.add_space(4.0);
                });
            out.content_size.y > list_rect.height() + 1.0
        })
        .inner;

    // 只在内容确实超出一屏时才画底部渐隐 + 分隔线：
    // 让「下面还有，可以滚」变得可读，而不是看着像卡片被切坏了。
    if overflow {
        let p = ui.painter();
        let fade_h = 26.0_f32.min(list_rect.height() * 0.25);
        let fade = egui::Rect::from_min_max(
            egui::pos2(list_rect.left(), list_bottom - fade_h),
            egui::pos2(list_rect.right(), list_bottom),
        );
        theme::fade_bottom(p, fade, P::BG);
        p.hline(
            list_rect.x_range(),
            list_bottom + 0.5,
            egui::Stroke::new(1.0, P::BORDER),
        );
    }

    // 脚注固定在底部区域（即便上方内容滚动也不会被压住）
    let foot_rect =
        egui::Rect::from_min_max(egui::pos2(avail.min.x, list_bottom), avail.max);
    ui.scope_builder(egui::UiBuilder::new().max_rect(foot_rect), |ui| {
        ui.set_clip_rect(foot_rect);
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            footer(ui);
        });
    });

    for a in acts {
        match a {
            Act::Download(pkg) => app.start_download(&pkg),
            Act::Resume => app.resume_download(),
            Act::ResolveLink(pkg) => app.resolve_link(&pkg),
            Act::ToggleAfter => app.toggle_after(),
        }
    }
}

// ---------------------------------------------------------------- 页头

fn header(ui: &mut egui::Ui, app: &mut App) {
    ui.add_space(18.0);
    ui.horizontal(|ui| {
        ui.add_space(PAD_X);
        let (name, code, brand) = match &app.current {
            Some(m) => (m.name.clone(), m.code.clone(), m.brand.clone()),
            None => ("未选择机型".to_string(), "—".to_string(), String::new()),
        };
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(&name).size(19.0).strong().color(P::TEXT));
                if !brand.is_empty() {
                    ui.add_space(1.0);
                    brand_pill(ui, &brand);
                }
            });
            ui.add_space(3.0);
            ui.label(
                egui::RichText::new(format!("机型代号 {}", code))
                    .size(11.2)
                    .monospace()
                    .color(P::TEXT_FAINT),
            );
        });

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(PAD_X);
            let can_rescan = app.current.is_some() && !app.querying;
            if can_rescan {
                if theme::ghost_button(ui, "重新扫描")
                    .on_hover_text("重新并发扫描该机型的所有分支")
                    .clicked()
                {
                    app.query_selected();
                }
            } else {
                theme::ghost_button_off(ui, "重新扫描")
                    .on_hover_text("选中机型后才能重新扫描");
            }
            if !app.packages.is_empty() {
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(format!("{} 个版本", app.packages.len()))
                        .size(11.5)
                        .color(P::TEXT_DIM),
                );
            }
        });
    });
    ui.add_space(14.0);
    divider(ui, PAD_X);
    ui.add_space(12.0);
}

// ---------------------------------------------------------------- 列表区块标题

/// 列表上方的分区标题：左边说明这是什么，右边说明排序与总数。
fn section_label(ui: &mut egui::Ui, n: usize, official: usize) {
    ui.horizontal(|ui| {
        ui.add_space(PAD_X);
        ui.label(
            egui::RichText::new("可用全量包")
                .size(12.0)
                .strong()
                .color(P::TEXT_DIM),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(PAD_X);
            ui.label(
                egui::RichText::new("按版本号倒序")
                    .size(10.8)
                    .color(P::with_alpha(P::TEXT_FAINT, 200)),
            );
            if n > official {
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(format!("社区 {}", n - official))
                        .size(10.8)
                        .color(P::with_alpha(P::VIOLET, 210)),
                );
            }
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(format!("官方 {}", official))
                    .size(10.8)
                    .color(P::with_alpha(P::TEXT_DIM, 220)),
            );
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(format!("共 {} 个", n))
                    .size(10.8)
                    .color(P::with_alpha(P::TEXT_FAINT, 200)),
            );
        });
    });
    ui.add_space(9.0);
}

// ---------------------------------------------------------------- 售后分区

/// 售后（降级 / 维修）包的分区标题。
///
/// 单独成段而不是混进版本清单：这些包的版本号是 ColorOS 自己的号
/// （ColorOS16.1.0），和升级包的「16.0.10.500」不在一个坐标系里，
/// 混排会让「按版本号倒序」这个承诺失效。
fn after_label(ui: &mut egui::Ui, n: usize, open: bool, shown: usize) -> Option<Act> {
    let out = ui.horizontal(|ui| {
        ui.add_space(PAD_X);
        ui.label(
            egui::RichText::new("售后全量包")
                .size(12.0)
                .strong()
                .color(P::TEXT_DIM),
        );
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new(format!("{} 个", n))
                .size(10.8)
                .color(P::with_alpha(P::WARN, 210)),
        );
        let mut act = None;
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(PAD_X);
            ui.label(
                egui::RichText::new("降级 / 维修用 · 可直连续传")
                    .size(10.8)
                    .color(P::with_alpha(P::TEXT_FAINT, 200)),
            );
            if n > shown || open {
                ui.add_space(10.0);
                let text = if open {
                    "收起".to_string()
                } else {
                    format!("展开全部 {}", n)
                };
                if theme::ghost_button_off_min(ui, &text, if open { 52.0 } else { 86.0 }).clicked() {
                    act = Some(Act::ToggleAfter);
                }
            }
        });
        act
    });
    out.response.on_hover_text(
        "售后全量包（降级 / 维修用完整固件），两个来源：\n\
         · 官方：OPPO 售后降级服务器按项目号下发，地址长期有效\n\
         · 社区：VioletToolBox 的售后包通道（角标「社区售后」），\n\
           一个机型常有三十多个版本，官方没下发降级包的机型靠它兜底\n\
         与升级包不是同一套版本序列，所以单独列出、不参与版本号排序。\n\
         线刷包（OFP）要用 EDL 工具走 9008 刷入，系统更新里刷不了。",
    );
    ui.add_space(9.0);
    out.inner
}

/// 售后通道本次没查通时的提示条。
///
/// 只在 `AfterState::Unknown` 时出现：`NoProject` 说明官方本来就没给这台机器
/// 提供售后包，说一句「没有」就够了，不该给出「再试一次」的暗示。
fn after_pending(ui: &mut egui::Ui) {
    let w = ui.available_width() - PAD_X * 2.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, 56.0), egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, theme::CR_CARD, P::with_alpha(P::WARN, 12));
    p.rect_stroke(
        rect,
        theme::CR_CARD,
        egui::Stroke::new(1.0, P::with_alpha(P::WARN, 60)),
        egui::StrokeKind::Inside,
    );

    let inner = rect.shrink2(egui::vec2(14.0, 0.0));
    let cy = rect.center().y;
    p.text(
        egui::pos2(inner.left(), cy - 8.0),
        egui::Align2::LEFT_CENTER,
        "售后全量包这次没查到",
        egui::FontId::proportional(12.6),
        P::WARN,
    );
    p.text(
        egui::pos2(inner.left(), cy + 10.0),
        egui::Align2::LEFT_CENTER,
        "OPPO 售后服务器偶发无响应，点右上角「重新扫描」多试一次通常就有了",
        egui::FontId::proportional(10.8),
        P::with_alpha(P::TEXT_DIM, 220),
    );
    ui.add_space(14.0);
}

// ---------------------------------------------------------------- 脚注

fn footer(ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.add_space(PAD_X);
        ui.label(
            egui::RichText::new("版本清单：OPPO 官方 OTA + VioletToolBox 社区库 + 售后通道")
                .size(10.8)
                .color(P::with_alpha(P::TEXT_FAINT, 205)),
        )
        .on_hover_text(
            "官方源：OPPO 服务器返回的版本与直链。\n\
             社区源：VioletToolBox，作者 Smart-Paocai，GNU GPL v3.0\n\
             https://github.com/Smart-Paocai/VioletToolBox\n\
             售后源：OPPO 售后降级服务器 downgrade.coloros.com（维修站固件，长期直链）\n\
             社区售后源：VioletToolBox 的 afterSales 通道（EDL 线刷包与 A/B 全量包）\n\
             本工具仅调用上述公开接口，未复制任何第三方源代码。",
        );
        ui.add_space(8.0);
        ui.label(
            // 官方签名链 10 分钟；社区与售后给的是长期裸链，统称「有效期」会讲错，
            // 所以只对官方那一类说时间，其余劝一句「失效就再点一次」。
            egui::RichText::new("· 官方直链约 10 分钟有效 · 失效就再点一次 · 下载后可核对 MD5")
                .size(10.8)
                .color(P::with_alpha(P::TEXT_FAINT, 170)),
        );
    });
}

// ---------------------------------------------------------------- 状态

fn brand_pill(ui: &mut egui::Ui, brand: &str) {
    theme::pill(ui, brand, theme::brand_color(brand));
}

fn loading(ui: &mut egui::Ui) {
    centered_block(ui, FOOTER_H, |ui| {
        ui.spinner();
        ui.add_space(14.0);
        ui.label(
            egui::RichText::new("正在并发扫描 A–Z 全部分支…")
                .size(13.0)
                .color(P::TEXT_DIM),
        );
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new("首次扫描通常需要几秒")
                .size(11.3)
                .color(P::TEXT_FAINT),
        );
    });
}

fn empty(ui: &mut egui::Ui, app: &App) {
    centered_block(ui, FOOTER_H, |ui| {
        empty_art(ui);
        ui.add_space(18.0);
        let t = if app.current.is_none() {
            "先从左侧选择一个机型"
        } else {
            "该机型当前没有可查询到的全量包"
        };
        ui.label(egui::RichText::new(t).size(15.0).color(P::TEXT_DIM));
        ui.add_space(8.0);
        let sub = if app.current.is_none() {
            "输入机型名称或代号后回车，例如 GT5 Pro、Find X8、RMX3888"
        } else {
            "海外版代号或已停更的机型通常查不到数据"
        };
        ui.label(egui::RichText::new(sub).size(11.8).color(P::TEXT_FAINT));
    });
}

/// 在剩余空间（扣除脚注）里垂直居中放一块内容，并把这块空间整段占掉，
/// 这样脚注始终贴在面板底部。
fn centered_block(ui: &mut egui::Ui, footer_h: f32, add: impl FnOnce(&mut egui::Ui)) {
    let w = ui.available_width();
    let h = (ui.available_height() - footer_h).max(120.0);

    // 先整段占位，再在占到的矩形里摆内容
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, h), egui::Sense::hover());
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::top_down(egui::Align::Center)),
    );
    child.add_space((h * 0.30).min(160.0));
    add(&mut child);
}

/// 空态插图：一个虚化圆底 + 圆角方块 + 下载箭头，克制不掉价。
fn empty_art(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(96.0, 96.0), egui::Sense::hover());
    let p = ui.painter();
    let c = rect.center();

    p.circle_filled(c, 44.0, P::with_alpha(P::ACCENT, 10));
    p.circle_filled(c, 32.0, P::with_alpha(P::ACCENT, 16));

    let box_r = egui::Rect::from_center_size(c, egui::vec2(34.0, 34.0));
    p.rect(
        box_r,
        egui::CornerRadius::same(10),
        P::with_alpha(P::ACCENT, 22),
        egui::Stroke::new(1.0, P::with_alpha(P::ACCENT, 84)),
        egui::StrokeKind::Inside,
    );

    let s = egui::Stroke::new(1.8, P::with_alpha(P::ACCENT, 235));
    p.line_segment([egui::pos2(c.x, c.y - 8.0), egui::pos2(c.x, c.y + 3.0)], s);
    p.line_segment([egui::pos2(c.x - 5.0, c.y - 2.5), egui::pos2(c.x, c.y + 3.5)], s);
    p.line_segment([egui::pos2(c.x + 5.0, c.y - 2.5), egui::pos2(c.x, c.y + 3.5)], s);
    p.line_segment([egui::pos2(c.x - 7.0, c.y + 8.0), egui::pos2(c.x + 7.0, c.y + 8.0)], s);
}

// ---------------------------------------------------------------- 卡片

fn card(ui: &mut egui::Ui, app: &App, pkg: &Package, latest: bool) -> Option<Act> {
    // 按包身份而不是版本号判断：售后包补齐真名后版本号和官方卡完全相同，
    // 用版本号比会让两张卡同时亮成「已复制」。
    // 已复制状态下要按源说明「这份链接能放多久」：三种源的有效期规则不同，
    // 一律说「约 10 分钟」对售后直链就是错的（它长期有效）。
    let link_src = app
        .link
        .as_ref()
        .filter(|l| l.id == pkg.id())
        .map(|l| l.source);
    let linked = link_src.is_some();
    let busy = app.is_downloading_pkg(pkg);
    let paused = app.is_paused_pkg(pkg);
    let rail = theme::branch_color(&pkg.branch);
    let style = if busy {
        theme::CardStyle::active(rail)
    } else if latest {
        theme::CardStyle::latest(rail)
    } else {
        theme::CardStyle::rail(rail)
    };

    let mut act = None;
    ui.horizontal(|ui| {
        ui.add_space(PAD_X);
        theme::card_with(ui, style, |ui| {
            // 右侧按钮组先占位：左侧文字区宽度被限制在剩余空间内，
            // 窄窗口下文字不会钻到按钮下面（这是最小窗口宽度下的实际 bug）。
            let reserved = BTN_W + 8.0 + BTN_W2 + 12.0;
            let left_w = (ui.available_width() - reserved).max(150.0);

            ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(left_w, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_max_width(left_w);

                        // 第一行：版本号 + 分支药丸（版本号过长时截断）
                        ui.horizontal(|ui| {
                            let ver = ui.add(
                                egui::Label::new(
                                    egui::RichText::new(&pkg.version)
                                        .size(15.5)
                                        .strong()
                                        .color(P::TEXT),
                                )
                                .truncate(),
                            );
                            if !pkg.md5.is_empty() {
                                ver.on_hover_text(format!("MD5\n{}", pkg.md5));
                            }
                            if !pkg.branch.is_empty() {
                                theme::pill(ui, &format!("{} 分支", pkg.branch), rail);
                            }
                            // 来源角标：社区源的版本必须能一眼看出来
                            let sc = match pkg.source {
                                Source::Official => P::TEXT_FAINT,
                                Source::Community => P::VIOLET,
                                Source::AfterSale => P::WARN,
                                // 社区收录的售后包同时属于两类，取售后那一路的暖色，
                                // 让它在售后分区里与官方售后卡同色系，不打断阅读。
                                Source::AfterCommunity => P::WARN,
                            };
                            theme::soft_pill(ui, pkg.source.label(), sc)
                                .on_hover_text(pkg.source.tip());
                            // 售后包里服务器给的用途说明（如「Enzo降级包」）
                            if !pkg.note.is_empty() {
                                theme::soft_pill(ui, &pkg.note, P::WARN)
                                    .on_hover_text("售后服务器标注的包用途");
                            }
                            // 封装形态：线刷包丢进系统更新里是刷不进去的，
                            // 必须在下载之前就让人看见。
                            if pkg.pack == crate::core::meta::Pack::Ofp {
                                theme::soft_pill(ui, pkg.pack.label(), P::DANGER)
                                    .on_hover_text(pkg.pack.tip());
                            } else if pkg.pack == crate::core::meta::Pack::Ab
                                && pkg.source == Source::AfterCommunity
                            {
                                // 社区售后卡两路混在一起，AB 也要标出来才好选；
                                // 官方卡本来就是 AB，标了反而噪音。
                                theme::soft_pill(ui, pkg.pack.label(), P::TEXT_DIM)
                                    .on_hover_text(pkg.pack.tip());
                            }
                            if latest {
                                ui.add_space(1.0);
                                theme::soft_pill(ui, "最新", P::OK);
                            }
                        });

                        ui.add_space(7.0);

                        // 第二行：元信息；空间不够时先舍掉「补丁」，保证不换行不重叠
                        ui.horizontal(|ui| {
                            let android = if pkg.android.is_empty() {
                                "—"
                            } else {
                                pkg.android.as_str()
                            };
                            meta(ui, "系统", android);
                            dot(ui);
                            meta(ui, "大小", &bytes(pkg.size));
                            if !pkg.published.is_empty() {
                                dot(ui);
                                meta(
                                    ui,
                                    "发布",
                                    &date(pkg.published.parse::<i64>().unwrap_or(0)),
                                );
                            }
                            if left_w >= 430.0 && !pkg.security_patch.is_empty() {
                                dot(ui);
                                meta(ui, "补丁", &pkg.security_patch);
                            }
                            // 社区包的直链由第三方站点签发，可用性必须先探测再展示
                            if pkg.link_ok == Some(false) {
                                dot(ui);
                                let t = ui.label(
                                    egui::RichText::new("链接已失效")
                                        .size(11.0)
                                        .color(P::WARN),
                                );
                                t.on_hover_text(
                                    "社区源记录了这个版本，但它签发的地址测试时拒绝访问\n\
                                     （旧版本有一部分地址已不再对外提供）",
                                );
                            }
                        });
                    },
                );

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if busy {
                        // 进行中：弱化态，视觉上明确「已在这个版本上工作」
                        theme::busy_button_min(ui, "下载中", BTN_W)
                            .on_hover_text("该版本正在下载，进度见底栏");
                    } else if paused {
                        // 这个版本暂停在手：主按钮变成「继续」，不必再回底栏找
                        if theme::primary_button_min(ui, "继续", BTN_W)
                            .on_hover_text("从已下载的分块继续，不会从头开始")
                            .clicked()
                        {
                            act = Some(Act::Resume);
                        }
                    } else if theme::primary_button_min(ui, "下载", BTN_W)
                        .on_hover_text(if pkg.link_ok == Some(false) {
                            // 探测只说「这一次没连上」，网络抖动也会失败，
                            // 所以不锁按钮——让用户自己决定要不要再试一次。
                            "该版本的地址在本次探测时未响应，仍可尝试下载"
                        } else {
                            "开始多线程分段下载，支持断点续传"
                        })
                        .clicked()
                    {
                        act = Some(Act::Download(pkg.clone()));
                    }
                    ui.add_space(8.0);
                    let hint = match (linked, link_src) {
                        (true, Some(Source::AfterSale)) => {
                            "已复制到剪贴板 · 售后直链长期有效，可直接粘给下载器"
                        }
                        (true, Some(Source::AfterCommunity)) => {
                            "已复制到剪贴板 · 社区收录的售后直链（失效就再点一次）"
                        }
                        (true, Some(Source::Community)) => {
                            "已复制到剪贴板 · 社区源直链，约 10 分钟内有效"
                        }
                        (true, Some(Source::Official)) => {
                            "已复制到剪贴板 · 官方签名直链，约 10 分钟内有效"
                        }
                        (true, None) => "已复制到剪贴板",
                        (false, _) if pkg.source == Source::AfterSale => {
                            "售后直链长期有效，点一下复制到剪贴板"
                        }
                        (false, _) if pkg.source == Source::AfterCommunity => {
                            "向社区源换取售后直链并复制"
                        }
                        _ => "解析官方直链并复制到剪贴板（约 10 分钟有效）",
                    };
                    if theme::ghost_button_min(ui, if linked { "已复制" } else { "直链" }, BTN_W2)
                        .on_hover_text(hint)
                        .clicked()
                    {
                        act = Some(Act::ResolveLink(pkg.clone()));
                    }
                });
            });
        });
    });
    act
}

fn meta(ui: &mut egui::Ui, k: &str, v: &str) {
    ui.label(egui::RichText::new(k).size(11.0).color(P::TEXT_FAINT));
    ui.add_space(3.0);
    ui.label(
        egui::RichText::new(v)
            .size(11.8)
            .monospace()
            .color(P::TEXT_DIM),
    );
}

fn dot(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(14.0, 10.0), egui::Sense::hover());
    ui.painter()
        .circle_filled(rect.center(), 1.6, P::BORDER_STRONG);
}

fn divider(ui: &mut egui::Ui, pad: f32) {
    ui.horizontal(|ui| {
        ui.add_space(pad);
        let w = ui.available_width() - pad;
        let (rect, _) = ui.allocate_exact_size(egui::vec2(w, 1.0), egui::Sense::hover());
        ui.painter()
            .hline(rect.x_range(), rect.center().y, egui::Stroke::new(1.0, P::BORDER));
    });
}


