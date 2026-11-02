//! 主题与自绘组件
//!
//! 深色专业工具风格：冷调中性黑底 + 低对比细边框 + 蓝色强调。
//! 所有颜色集中在 `P`，所有自绘组件只依赖 egui 0.37 的公开 API。
//!
//! 用法：在 App::new 里调用一次 `theme::apply(ctx)`，
//! 之后直接用 `card` / `pill` / `primary_button` / `progress_bar` 等组件。

use eframe::egui;

// ============================================================================
//  调色板
// ============================================================================

/// 全局调色板：所有颜色常量的唯一来源。
pub struct P;

impl P {
    /// 主背景 #0E1116（冷调中性黑）。
    pub const BG: egui::Color32 = egui::Color32::from_rgb(0x0E, 0x11, 0x16);
    /// 侧栏 / 次级面板背景 #0A0D12，比主背景略深。
    pub const BG_SIDE: egui::Color32 = egui::Color32::from_rgb(0x0A, 0x0D, 0x12);
    /// 卡片背景 #151A22，比底色稍亮。
    pub const CARD: egui::Color32 = egui::Color32::from_rgb(0x15, 0x1A, 0x22);
    /// 卡片 hover 背景 #1C232D。
    pub const CARD_HOVER: egui::Color32 = egui::Color32::from_rgb(0x1C, 0x23, 0x2D);
    /// 常规细边框 #232A34（低对比 1px）。
    pub const BORDER: egui::Color32 = egui::Color32::from_rgb(0x23, 0x2A, 0x34);
    /// 强化边框 / hover 边框 #3A4552。
    pub const BORDER_STRONG: egui::Color32 = egui::Color32::from_rgb(0x3A, 0x45, 0x52);
    /// 主文字 #E6EAF0。
    pub const TEXT: egui::Color32 = egui::Color32::from_rgb(0xE6, 0xEA, 0xF0);
    /// 次要文字 #9BA5B4。
    pub const TEXT_DIM: egui::Color32 = egui::Color32::from_rgb(0x9B, 0xA5, 0xB4);
    /// 最弱文字 / 提示 #6B7686。
    pub const TEXT_FAINT: egui::Color32 = egui::Color32::from_rgb(0x6B, 0x76, 0x86);
    /// 强调蓝 #4C8DFF。
    pub const ACCENT: egui::Color32 = egui::Color32::from_rgb(0x4C, 0x8D, 0xFF);
    /// 强调蓝悬停提亮 #6BA1FF（主按钮 hover 用）。
    pub const ACCENT_HOVER: egui::Color32 = egui::Color32::from_rgb(0x6B, 0xA1, 0xFF);
    /// 强调蓝按下 / 深色 #2F6BDA。
    pub const ACCENT_DEEP: egui::Color32 = egui::Color32::from_rgb(0x2F, 0x6B, 0xDA);
    /// 成功 #34C77B。
    pub const OK: egui::Color32 = egui::Color32::from_rgb(0x34, 0xC7, 0x7B);
    /// 警告 #F2A33C。
    pub const WARN: egui::Color32 = egui::Color32::from_rgb(0xF2, 0xA3, 0x3C);
    /// 危险 / 错误 #F05B5B。
    pub const DANGER: egui::Color32 = egui::Color32::from_rgb(0xF0, 0x5B, 0x5B);
    /// 紫罗兰 #8B5CF6，渐变终点与次级强调。
    pub const VIOLET: egui::Color32 = egui::Color32::from_rgb(0x8B, 0x5C, 0xF6);

    /// 以 ACCENT 为基色、指定 alpha 的弱化强调色（选中底、focus 光环等）。
    pub fn accent_soft(alpha: u8) -> egui::Color32 {
        P::with_alpha(P::ACCENT, alpha)
    }

    /// 给任意颜色替换 alpha（按 Color32 预乘规则换算，已是半透明色也不会二次变暗）。
    pub fn with_alpha(c: egui::Color32, a: u8) -> egui::Color32 {
        let [r, g, b, ca] = c.to_array();
        if ca == 0 {
            return egui::Color32::TRANSPARENT;
        }
        if ca == 255 {
            return egui::Color32::from_rgba_unmultiplied(r, g, b, a);
        }
        // 先反预乘回原始分量，再按新 alpha 重新预乘
        let un = |v: u8| -> u8 {
            let f = (v as f32) * 255.0 / (ca as f32);
            f.round().clamp(0.0, 255.0) as u8
        };
        egui::Color32::from_rgba_unmultiplied(un(r), un(g), un(b), a)
    }
}

// ============================================================================
//  尺寸常量
// ============================================================================

/// 卡片圆角。
/// 卡片 / 弹窗圆角
pub const CR_CARD: u8 = 12;
/// 按钮圆角。
const CR_BUTTON: u8 = 9;

// ============================================================================
//  主题装配
// ============================================================================

/// 在 App::new 里调用一次：装中文字体 + 深色主题 + 圆角/间距 + 文本样式。
pub fn apply(ctx: &egui::Context) {
    install_fonts(ctx);
    ctx.set_theme(egui::ThemePreference::Dark);

    ctx.all_styles_mut(|style| {
        let mut v = egui::Visuals::dark();

        v.dark_mode = true;
        v.override_text_color = Some(P::TEXT);
        v.weak_text_alpha = 0.72;
        v.hyperlink_color = P::ACCENT;
        v.warn_fg_color = P::WARN;
        v.error_fg_color = P::DANGER;

        // 窗口 / 面板
        v.panel_fill = P::BG;
        v.window_fill = P::CARD;
        v.window_stroke = egui::Stroke::new(1.0, P::BORDER);
        v.window_corner_radius = egui::CornerRadius::same(CR_CARD);
        v.window_highlight_topmost = false;
        v.menu_corner_radius = egui::CornerRadius::same(10);

        // 各种内嵌背景
        v.faint_bg_color = P::CARD;
        v.extreme_bg_color = P::BG_SIDE;
        v.code_bg_color = P::BG_SIDE;
        v.text_edit_bg_color = Some(P::BG_SIDE);

        // 文本选区
        v.selection.bg_fill = P::accent_soft(72);
        v.selection.stroke = egui::Stroke::new(1.0, P::ACCENT);

        // 控件五态：noninteractive / inactive / hovered / active / open
        {
            let w = &mut v.widgets;

            w.noninteractive.bg_fill = P::CARD;
            w.noninteractive.weak_bg_fill = P::CARD;
            w.noninteractive.bg_stroke = egui::Stroke::new(1.0, P::BORDER);
            w.noninteractive.fg_stroke = egui::Stroke::new(1.0, P::TEXT_DIM);
            w.noninteractive.corner_radius = egui::CornerRadius::same(CR_BUTTON);
            w.noninteractive.expansion = 0.0;

            w.inactive.bg_fill = P::CARD;
            w.inactive.weak_bg_fill = P::CARD;
            w.inactive.bg_stroke = egui::Stroke::new(1.0, P::BORDER);
            w.inactive.fg_stroke = egui::Stroke::new(1.0, P::TEXT);
            w.inactive.corner_radius = egui::CornerRadius::same(CR_BUTTON);
            w.inactive.expansion = 0.0;

            w.hovered.bg_fill = P::CARD_HOVER;
            w.hovered.weak_bg_fill = P::CARD_HOVER;
            w.hovered.bg_stroke = egui::Stroke::new(1.0, P::BORDER_STRONG);
            w.hovered.fg_stroke = egui::Stroke::new(1.0, egui::Color32::WHITE);
            w.hovered.corner_radius = egui::CornerRadius::same(CR_BUTTON);
            w.hovered.expansion = 0.0;

            w.active.bg_fill = P::accent_soft(48);
            w.active.weak_bg_fill = P::accent_soft(48);
            w.active.bg_stroke = egui::Stroke::new(1.0, P::ACCENT);
            w.active.fg_stroke = egui::Stroke::new(1.0, egui::Color32::WHITE);
            w.active.corner_radius = egui::CornerRadius::same(CR_BUTTON);
            w.active.expansion = 0.0;

            w.open.bg_fill = P::CARD_HOVER;
            w.open.weak_bg_fill = P::CARD_HOVER;
            w.open.bg_stroke = egui::Stroke::new(1.0, P::BORDER_STRONG);
            w.open.fg_stroke = egui::Stroke::new(1.0, egui::Color32::WHITE);
            w.open.corner_radius = egui::CornerRadius::same(CR_BUTTON);
            w.open.expansion = 0.0;
        }

        style.visuals = v;

        // 间距：舒服的按钮内边距 + 至少 30px 的交互高度
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(14.0, 8.0);
        style.spacing.interact_size = egui::vec2(0.0, 30.0);
        style.spacing.window_margin = egui::Margin::same(12);
        style.spacing.indent = 18.0;
        style.spacing.text_edit_width = 180.0;

        // 文本样式
        style.text_styles.insert(
            egui::TextStyle::Body,
            egui::FontId::new(14.5, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Small,
            egui::FontId::new(12.5, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Button,
            egui::FontId::new(14.0, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Heading,
            egui::FontId::new(19.0, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Monospace,
            egui::FontId::new(13.5, egui::FontFamily::Monospace),
        );
    });
}

/// 中文字体候选路径，按优先级排列。
const FONT_CANDIDATES: [&str; 5] = [
    r"C:\Windows\Fonts\msyh.ttc",
    r"C:\Windows\Fonts\msyhbd.ttc",
    r"C:\Windows\Fonts\simhei.ttf",
    r"C:\Windows\Fonts\Deng.ttf",
    r"C:\Windows\Fonts\simsun.ttc",
];

/// 读取第一个可用的中文字体文件，返回 (注册名, 字节)。
fn pick_font() -> Option<(String, Vec<u8>)> {
    for (i, path) in FONT_CANDIDATES.iter().enumerate() {
        if let Ok(bytes) = std::fs::read(path) {
            if bytes.len() > 1024 {
                return Some((format!("cn-font-{i}"), bytes));
            }
        }
    }
    None
}

/// 把系统中文字体装进 egui：Proportional 家族最前，Monospace 家族末尾兜底。
fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    if let Some((name, bytes)) = pick_font() {
        fonts.font_data.insert(
            name.clone(),
            std::sync::Arc::new(egui::FontData::from_owned(bytes)),
        );
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, name.clone());
        fonts
            .families
            .entry(egui::FontFamily::Monospace)
            .or_default()
            .push(name);
    }

    ctx.set_fonts(fonts);
}

// ============================================================================
//  颜色工具
// ============================================================================

/// 线性插值两个颜色（按 Color32 的预乘分量插值，不透明色之间结果最准）。
fn lerp_color(a: egui::Color32, b: egui::Color32, t: f32) -> egui::Color32 {
    let t = t.clamp(0.0, 1.0);
    let [ar, ag, ab, aa] = a.to_array();
    let [br, bg, bb, ba] = b.to_array();
    let mix = |x: u8, y: u8| -> u8 {
        let v = (x as f32) + ((y as f32) - (x as f32)) * t;
        v.round().clamp(0.0, 255.0) as u8
    };
    egui::Color32::from_rgba_premultiplied(mix(ar, br), mix(ag, bg), mix(ab, bb), mix(aa, ba))
}

/// 分支字母 A..Z 稳定映射到一组好看的颜色（同一字母永远同色）。
///
/// 刻意避开红色系：红色在本应用专指「错误 / 危险」，不能再用作分类色。
pub fn branch_color(branch: &str) -> egui::Color32 {
    const COLORS: [egui::Color32; 8] = [
        egui::Color32::from_rgb(0x4C, 0x8D, 0xFF),
        egui::Color32::from_rgb(0x34, 0xC7, 0x7B),
        egui::Color32::from_rgb(0xF2, 0xA3, 0x3C),
        egui::Color32::from_rgb(0x8B, 0x5C, 0xF6),
        egui::Color32::from_rgb(0x22, 0xB8, 0xCF),
        egui::Color32::from_rgb(0x5E, 0xEA, 0xD4),
        egui::Color32::from_rgb(0xE8, 0x79, 0xF9),
        egui::Color32::from_rgb(0xA3, 0xE6, 0x35),
    ];

    for ch in branch.chars() {
        if ch.is_ascii_alphabetic() {
            let idx = (ch.to_ascii_uppercase() as u8 - b'A') as usize;
            return COLORS[idx % COLORS.len()];
        }
    }

    P::TEXT_DIM
}

/// 品牌的代表色：realme 用强调蓝、一加用红、OPPO 用绿。
/// 侧栏头像与页头药丸共用，保证同一品牌在任何位置都是同一个色。
pub fn brand_color(brand: &str) -> egui::Color32 {
    match brand {
        "realme" => P::ACCENT,
        "OnePlus" => P::DANGER,
        _ => P::OK,
    }
}

/// 品牌头像里显示的字（OPPO / realme / 一加）。
pub fn brand_glyph(brand: &str) -> &'static str {
    match brand {
        "realme" => "真",
        "OnePlus" => "加",
        _ => "欧",
    }
}

// ============================================================================
//  卡片
// ============================================================================

/// 卡片外观：左侧强调条 + 「激活」态（底色带一点强调色微光，用于正在下载的卡片）。
///
/// 强调条与内边距由本结构体统一控制，调用方只选颜色，不再关心排版细节。
#[derive(Clone, Copy)]
pub struct CardStyle {
    /// 左侧 3px 竖条的颜色；None 表示不画。
    rail: Option<egui::Color32>,
    /// 激活态：底色混入强调色，边框换成强调色描边。
    active: bool,
    /// 最新版本：底色极轻地混入强调色，边框带一圈弱光，形成主次层次。
    latest: bool,
}

impl CardStyle {
    /// 带左侧强调条的卡片（按分支上色）。
    pub fn rail(c: egui::Color32) -> Self {
        Self { rail: Some(c), active: false, latest: false }
    }

    /// 最新版本的卡片：比其他卡片多一层极淡的强调色底 + 弱描边。
    /// 底色用主强调蓝而非分支色，避免被分支色（红/橙）染脏。
    pub fn latest(rail: egui::Color32) -> Self {
        Self { rail: Some(rail), active: false, latest: true }
    }

    /// 带强调条且处于激活态（正在下载）。
    pub fn active(c: egui::Color32) -> Self {
        Self { rail: Some(c), active: true, latest: false }
    }

    fn pad_x(&self) -> i8 {
        if self.rail.is_some() { 20 } else { 14 }
    }
}

/// 带样式的卡片：自绘底色 / 边框 / 左侧强调条，hover 与激活态都做平滑过渡。
pub fn card_with(
    ui: &mut egui::Ui,
    style: CardStyle,
    add: impl FnOnce(&mut egui::Ui),
) -> egui::Response {
    let cr = egui::CornerRadius::same(CR_CARD);

    // Frame 自身只负责布局（内边距 / 圆角）与返回区域；
    // 底色与边框改由本函数自绘：先占一个 Noop 形状槽位（位于内容之下），
    // 拿到区域后再把它填成卡片底色，这样 hover 时底色与边框都能平滑过渡。
    let painter = ui.painter().clone();
    let bg_idx = painter.add(egui::Shape::Noop);

    let inner = egui::Frame::new()
        .fill(egui::Color32::TRANSPARENT)
        .stroke(egui::Stroke::NONE)
        .corner_radius(cr)
        .inner_margin(egui::Margin::symmetric(style.pad_x(), 16))
        .show(ui, add);

    let resp = inner.response;
    let rect = resp.rect;
    let hovered = resp.hovered() || ui.rect_contains_pointer(rect);
    let t = ui.ctx().animate_bool(resp.id, hovered);

    // 底色：普通态 CARD → CARD_HOVER；最新版混入 3.5%，下载中混入 8% 强调色。
    let base = if style.active {
        // 下载中统一用强调蓝，避免分支色（红/橙）让「正在下载」读起来像错误
        lerp_color(P::CARD, P::ACCENT, 0.09)
    } else if style.latest {
        lerp_color(P::CARD, P::ACCENT, 0.045)
    } else {
        P::CARD
    };
    let hover_amt = if style.active { 0.5 } else { 1.0 };
    let fill = lerp_color(base, P::CARD_HOVER, t * hover_amt);
    painter.set(bg_idx, egui::Shape::rect_filled(rect, cr, fill));

    // 边框：下载中用强调色实描边，最新版用弱描边，其余按 hover 提亮。
    let border = match style.rail {
        Some(_) if style.active => egui::Stroke::new(1.0, P::with_alpha(P::ACCENT, 130)),
        Some(_) if style.latest => egui::Stroke::new(
            1.0,
            lerp_color(P::with_alpha(P::ACCENT, 60), P::with_alpha(P::ACCENT, 118), t),
        ),
        _ => egui::Stroke::new(1.0, lerp_color(P::BORDER, P::BORDER_STRONG, t)),
    };
    painter.rect_stroke(rect, cr, border, egui::StrokeKind::Inside);

    // 左侧强调条：上下各留 11px，圆角 2px，避免和卡片圆角打架。
    if let Some(c) = style.rail {
        let rail = egui::Rect::from_min_max(
            egui::pos2(rect.left() + 1.0, rect.top() + 11.0),
            egui::pos2(rect.left() + 4.0, rect.bottom() - 11.0),
        );
        // 下载中把竖条也统一成强调蓝，最新版用分支色高亮，其余弱化
        let (rail_color, alpha) = if style.active {
            (P::ACCENT, 255)
        } else if style.latest {
            (c, 235)
        } else {
            (c, 120)
        };
        painter.rect_filled(rail, egui::CornerRadius::same(2), P::with_alpha(rail_color, alpha));
    }

    resp
}

// ============================================================================
//  药丸标签
// ============================================================================

/// 圆角小药丸标签（描边款），如 "A16"、"8.76 GB"、"rmx3888"；返回 Response 便于 on_hover_text。
pub fn pill(ui: &mut egui::Ui, text: &str, color: egui::Color32) -> egui::Response {
    pill_impl(ui, text, color, false)
}

/// 实心圆角标签（背景 = color 的 18% alpha，文字 = color）。
pub fn soft_pill(ui: &mut egui::Ui, text: &str, color: egui::Color32) -> egui::Response {
    pill_impl(ui, text, color, true)
}

/// 药丸标签的统一实现（solid = 实心款，否则描边款）。
fn pill_impl(ui: &mut egui::Ui, text: &str, color: egui::Color32, solid: bool) -> egui::Response {
    // 先拿到布局，再分配空间（painter 先 clone，避免同时借用 ui）
    let painter = ui.painter().clone();
    let font = egui::FontId::new(12.0, egui::FontFamily::Proportional);
    let galley = painter.layout_no_wrap(text.to_owned(), font, color);

    let pad = egui::vec2(7.0, 3.0);
    let text_size = galley.size();
    let (rect, resp) = ui.allocate_exact_size(text_size + pad * 2.0, egui::Sense::hover());

    let cr = egui::CornerRadius::same(255); // u8 上限，等价于全圆角
    if solid {
        painter.rect_filled(rect, cr, P::with_alpha(color, 46));
    } else {
        painter.rect(
            rect,
            cr,
            P::with_alpha(color, 26),
            egui::Stroke::new(1.0, P::with_alpha(color, 110)),
            egui::StrokeKind::Inside,
        );
    }

    let pos = rect.center() - text_size * 0.5;
    painter.galley(pos, galley, color);

    resp
}

// ============================================================================
//  按钮
// ============================================================================

/// 主按钮（ACCENT 实心，hover 提亮，按下加深）。
pub fn primary_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    primary_button_min(ui, text, 0.0)
}

/// 主按钮，可指定最小宽度（同一列按钮宽度固定，切换文字时不抖动）。
pub fn primary_button_min(ui: &mut egui::Ui, text: &str, min_w: f32) -> egui::Response {
    styled_button(
        ui,
        text,
        min_w,
        P::ACCENT,
        P::ACCENT_HOVER,
        P::ACCENT_DEEP,
        egui::Stroke::NONE,
        egui::Stroke::NONE,
        egui::Color32::WHITE,
    )
}

/// 次按钮（透明底 + 细边框，hover 微微提亮）。
pub fn ghost_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ghost_button_min(ui, text, 0.0)
}

/// 次按钮，可指定最小宽度。
pub fn ghost_button_min(ui: &mut egui::Ui, text: &str, min_w: f32) -> egui::Response {
    styled_button(
        ui,
        text,
        min_w,
        egui::Color32::TRANSPARENT,
        P::with_alpha(egui::Color32::WHITE, 14),
        P::with_alpha(egui::Color32::WHITE, 24),
        egui::Stroke::new(1.0, P::BORDER),
        egui::Stroke::new(1.0, P::BORDER_STRONG),
        P::TEXT,
    )
}

/// 禁用态次按钮：无交互、文字压暗，用于「当前不可用」的操作。
pub fn ghost_button_off(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ghost_button_off_min(ui, text, 0.0)
}

/// 禁用态次按钮，可指定最小宽度（与相邻按钮等宽，避免排版抖动）。
pub fn ghost_button_off_min(ui: &mut egui::Ui, text: &str, min_w: f32) -> egui::Response {
    styled_button(
        ui,
        text,
        min_w,
        egui::Color32::TRANSPARENT,
        egui::Color32::TRANSPARENT,
        egui::Color32::TRANSPARENT,
        egui::Stroke::new(1.0, P::with_alpha(P::BORDER, 150)),
        egui::Stroke::new(1.0, P::with_alpha(P::BORDER, 150)),
        P::with_alpha(P::TEXT_FAINT, 170),
    )
}

/// 进行中按钮：占用主按钮位置但整体弱化，表示「已在处理、暂不可再点」。
pub fn busy_button_min(ui: &mut egui::Ui, text: &str, min_w: f32) -> egui::Response {
    styled_button(
        ui,
        text,
        min_w,
        P::with_alpha(P::ACCENT, 30),
        P::with_alpha(P::ACCENT, 30),
        P::with_alpha(P::ACCENT, 30),
        egui::Stroke::new(1.0, P::with_alpha(P::ACCENT, 90)),
        egui::Stroke::new(1.0, P::with_alpha(P::ACCENT, 90)),
        P::with_alpha(P::ACCENT, 235),
    )
}

/// 危险 / 取消按钮（红系，弱填充 + 红边框）。
pub fn danger_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    styled_button(
        ui,
        text,
        0.0,
        P::with_alpha(P::DANGER, 34),
        P::with_alpha(P::DANGER, 74),
        P::with_alpha(P::DANGER, 110),
        egui::Stroke::new(1.0, P::with_alpha(P::DANGER, 90)),
        egui::Stroke::new(1.0, P::DANGER),
        P::DANGER,
    )
}

/// 按钮统一实现：自绘圆角底 + 居中文字，四态配色完全可控。
#[allow(clippy::too_many_arguments)]
fn styled_button(
    ui: &mut egui::Ui,
    text: &str,
    min_w: f32,
    fill: egui::Color32,
    fill_hover: egui::Color32,
    fill_press: egui::Color32,
    border: egui::Stroke,
    border_hover: egui::Stroke,
    text_color: egui::Color32,
) -> egui::Response {
    let font = egui::FontId::new(14.0, egui::FontFamily::Proportional);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, text_color);

    let text_size = galley.size();
    let pad = egui::vec2(16.0, 7.0);
    let size = egui::vec2(
        (text_size.x + pad.x * 2.0).max(min_w),
        text_size.y.max(18.0) + pad.y * 2.0,
    );

    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());

    let (bg, stroke) = if resp.is_pointer_button_down_on() {
        (fill_press, border_hover)
    } else if resp.hovered() {
        (fill_hover, border_hover)
    } else {
        (fill, border)
    };

    let painter = ui.painter();
    painter.rect(rect, egui::CornerRadius::same(CR_BUTTON), bg, stroke, egui::StrokeKind::Inside);

    let pos = rect.center() - text_size * 0.5;
    painter.galley(pos, galley, text_color);

    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    resp
}

// ============================================================================
//  文本 / 分隔
// ============================================================================

// ============================================================================
//  自绘图形：渐变 / 进度条
// ============================================================================

/// 生成圆角矩形的水平渐变网格（左 a → 右 b），用扇形三角化保证圆角正确。
fn rounded_gradient_mesh(
    rect: egui::Rect,
    radius: f32,
    a: egui::Color32,
    b: egui::Color32,
) -> egui::Mesh {
    let mut mesh = egui::Mesh::default();

    let w = rect.width();
    let r = radius.max(0.0).min(w * 0.5).min(rect.height() * 0.5);

    let color_at = |x: f32| -> egui::Color32 {
        let t = if w > 0.0 {
            ((x - rect.left()) / w).clamp(0.0, 1.0)
        } else {
            0.0
        };
        lerp_color(a, b, t)
    };

    // 直角情况：4 顶点 2 三角形
    if r < 0.5 {
        mesh.colored_vertex(rect.left_top(), a);
        mesh.colored_vertex(rect.right_top(), b);
        mesh.colored_vertex(rect.left_bottom(), a);
        mesh.colored_vertex(rect.right_bottom(), b);
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(1, 3, 2);
        return mesh;
    }

    // 每个圆角的分段数
    const SEGS: usize = 6;
    // 四分之一圆
    const QUARTER: f32 = std::f32::consts::FRAC_PI_2;

    // 四个圆角的圆心：右上、右下、左下、左上（顺时针）
    let centers = [
        egui::pos2(rect.right() - r, rect.top() + r),
        egui::pos2(rect.right() - r, rect.bottom() - r),
        egui::pos2(rect.left() + r, rect.bottom() - r),
        egui::pos2(rect.left() + r, rect.top() + r),
    ];

    // 顶点 0 = 扇形中心
    let c = rect.center();
    mesh.colored_vertex(c, color_at(c.x));

    for (ci, center) in centers.iter().enumerate() {
        let base = -QUARTER + ci as f32 * QUARTER;
        for s in 0..=SEGS {
            let ang = base + (s as f32 / SEGS as f32) * QUARTER;
            let pos = egui::pos2(center.x + ang.cos() * r, center.y + ang.sin() * r);
            mesh.colored_vertex(pos, color_at(pos.x));
        }
    }

    let n = (centers.len() * (SEGS + 1)) as u32;
    for i in 0..n {
        mesh.add_triangle(0, 1 + i, 1 + (i + 1) % n);
    }

    mesh
}

/// 水平渐变填充圆角矩形（顶部标题栏 / 进度条强调色带）。
pub fn gradient_rect(
    painter: &egui::Painter,
    rect: egui::Rect,
    a: egui::Color32,
    b: egui::Color32,
    radius: u8,
) {
    if rect.width() < 1.0 || rect.height() < 1.0 {
        return;
    }
    let mesh = rounded_gradient_mesh(rect, radius as f32, a, b);
    painter.add(egui::Shape::mesh(mesh));
}

/// 竖直方向的渐隐遮罩：从透明渐变到底色，用于列表底部被截断处的柔和过渡。
///
/// `solid_at_bottom` = true 表示底部为实色（贴住脚注），顶部透明。
pub fn fade_bottom(painter: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
    if rect.width() < 1.0 || rect.height() < 1.0 {
        return;
    }
    let clear = egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 0);
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(rect.left_top(), clear);
    mesh.colored_vertex(rect.right_top(), clear);
    mesh.colored_vertex(rect.left_bottom(), color);
    mesh.colored_vertex(rect.right_bottom(), color);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(1, 3, 2);
    painter.add(egui::Shape::mesh(mesh));
}

/// 细进度条（自绘、圆角、渐变），frac 0..=1；frac 为 0 时只画底槽。
///
/// 前景带一条随时间平移的高光，暗示「仍在传输」，静止画面也能看出在动。
pub fn progress_bar(ui: &mut egui::Ui, frac: f32, width: f32, height: f32) -> egui::Response {
    progress_bar_animated(ui, frac, width, height, true)
}

/// 同上，可关闭流动高光（进度已完成时用，避免误导）。
pub fn progress_bar_static(ui: &mut egui::Ui, frac: f32, width: f32, height: f32) -> egui::Response {
    progress_bar_animated(ui, frac, width, height, false)
}

fn progress_bar_animated(
    ui: &mut egui::Ui,
    frac: f32,
    width: f32,
    height: f32,
    animated: bool,
) -> egui::Response {
    let frac = frac.clamp(0.0, 1.0);
    let h = height.max(2.0);

    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, h), egui::Sense::hover());
    let radius = ((h * 0.5).round() as u8).max(1);
    let cr = egui::CornerRadius::same(radius);

    // 底槽
    ui.painter().rect_filled(rect, cr, P::BORDER);

    // 前景：渐变条；极短时至少保留一个圆头宽度，避免出现畸形窄条
    if frac > 0.0 {
        let fg_w = (rect.width() * frac).max(h).min(rect.width());
        let fg = egui::Rect::from_min_size(rect.min, egui::vec2(fg_w, rect.height()));
        gradient_rect(ui.painter(), fg, P::ACCENT, P::VIOLET, radius);

        if animated && fg_w > 36.0 {
            // 高光条在已完成区间里循环右移，宽度取前景的 18%
            let t = ui.input(|i| i.time) as f32;
            let phase = (t * 0.42).fract();
            let band_w = (fg_w * 0.18).clamp(16.0, 90.0);
            let x = fg.left() - band_w + (fg_w + band_w) * phase;
            let band = egui::Rect::from_min_max(
                egui::pos2(x, fg.top()),
                egui::pos2(x + band_w, fg.bottom()),
            );
            let band = band.intersect(fg);
            if band.width() > 0.5 {
                ui.painter().rect_filled(
                    band,
                    egui::CornerRadius::same(radius),
                    P::with_alpha(egui::Color32::WHITE, 34),
                );
            }
            ui.ctx().request_repaint();
        }
    }

    resp
}
