//! 社区源：VioletToolBox 的 ROM 版本清单
//!
//! 数据来源与致谢：
//! - 项目：VioletToolBox，作者 Smart-Paocai
//! - 仓库：https://github.com/Smart-Paocai/VioletToolBox  （GNU GPL v3.0）
//! - 服务：https://violettool.top/rom-api
//!
//! 本模块只调用上述服务的公开 HTTP 接口（无需鉴权、无需登录），
//! 不复制、不修改、不分发其源代码，仅转发查询版本清单与下载地址。
//!
//! 该服务有两个通道，同一个机型在两边是完全不同的两批文件，必须分别查：
//! - `full`：常规版本清单（里面零星夹着标注「降级包」的回退包）；
//! - `afterSales`：售后包，量很大（一个机型常有 30 条以上），封装分两种 ——
//!   A/B 全量包与 OFP / EDL 线刷包，后者的刷入方式与普通升级流程完全不同。

use crate::core::meta;
use crate::core::ota::{self, Package, Source};
use serde_json::Value;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 社区源作者
pub const AUTHOR: &str = "Smart-Paocai";
/// 社区源项目主页
pub const REPO: &str = "https://github.com/Smart-Paocai/VioletToolBox";
/// 社区源站点
pub const SITE: &str = "https://violettool.top";
/// 社区源接口地址
pub const API: &str = "https://violettool.top/rom-api";
/// 社区源许可证
pub const LICENSE: &str = "GNU GPL v3.0";

/// 目录缓存有效期（秒）
const CACHE_TTL: u64 = 86_400;
const UA: &str = "ota-tool/1.0";

/// 常规版本清单通道
const KIND_FULL: &str = "full";
/// 售后包通道（含 A/B 全量包与 OFP / EDL 线刷包）
const KIND_AS: &str = "afterSales";

/// 名称归一化时要剥掉的海外版本标记。
///
/// 这些条目在售后通道里基本不存在（售后包只覆盖国行），先剥掉只是为了
/// 万一对方哪天补上时不至于把同一个型号算成两台机器。
const OVERSEAS: [&str; 16] = [
    "印度版",
    "欧洲版",
    "国际版",
    "北美版",
    "日版",
    "韩版",
    "东南亚",
    "俄罗斯",
    "中东",
    "拉美",
    "T-Mobile",
    "Metro",
    "Verizon",
    "AT&T",
    "Global",
    "全球版",
];

/// 名称归一化时要剥掉的尾部修饰词（按长度从长到短试，避免「定制版」先被「版」吃掉）
const AS_SUFFIX: [&str; 18] = [
    "原神定制礼盒",
    "运营商定制版",
    "电信合作版",
    "移动定制版",
    "全网通版",
    "国行版",
    "移动版",
    "联通版",
    "电信版",
    "定制版",
    "限量版",
    "限定版",
    "典藏版",
    "礼盒版",
    "套装版",
    "标准版",
    "5g",
    "4g",
];

/// 品牌词（售后通道的设备名不带型号码，必须靠品牌 + 主体名定位）
const BRAND_WORDS: [&str; 11] = [
    "真我", "一加", "欧加", "OPPO", "Oppo", "oppo", "Realme", "realme", "ONEPLUS", "OnePlus",
    "oneplus",
];

/// 社区源目录里的一条设备条目
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Device {
    pub brand: String,
    pub series: String,
    pub device: String,
}

/// 换取直链所需的四元组（原样回传给社区源）
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Ref {
    pub brand: String,
    pub series: String,
    pub device: String,
    pub version: String,
    /// 该条目属于哪个通道（`full` / `afterSales`）。
    ///
    /// 两个通道的设备名与版本名互不通用：拿售后通道的设备名去请求 full
    /// 一律 404，反过来也一样，所以换直链时必须原样带回通道名。
    /// 老版本留下的历史记录没有这个字段，反序列化成空串，按 full 处理。
    #[serde(default)]
    pub kind: String,
}

impl Ref {
    /// 通道名（老记录缺这个字段时按 full 处理）
    pub fn kind_of(&self) -> &str {
        if self.kind.is_empty() {
            KIND_FULL
        } else {
            self.kind.as_str()
        }
    }
}

// ---------------------------------------------------------------- 基础请求

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(8))
        .timeout_read(Duration::from_secs(15))
        .user_agent(UA)
        .build()
}

fn err_text(e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(c, _) => format!("HTTP {}", c),
        ureq::Error::Transport(t) => t.to_string(),
    }
}

/// 查询串用的百分号编码（只保留 URL 非保留字符）
fn enc(s: &str) -> String {
    let mut out = String::new();
    for b in s.as_bytes() {
        let c = *b as char;
        if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~') {
            out.push(c);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

fn items(agent: &ureq::Agent, url: &str) -> Result<Vec<Value>, String> {
    let resp = agent.get(url).call().map_err(err_text)?;
    let v: Value = resp.into_json().map_err(|e| e.to_string())?;
    Ok(v.get("items")
        .and_then(|x| x.as_array())
        .cloned()
        .unwrap_or_default())
}

fn get_strings(agent: &ureq::Agent, url: &str) -> Result<Vec<String>, String> {
    Ok(items(agent, url)?
        .iter()
        .filter_map(|i| i.as_str().map(|s| s.to_string()))
        .collect())
}

/// /versions 返回的是 [{"name": "..."}]
fn get_names(agent: &ureq::Agent, url: &str) -> Result<Vec<String>, String> {
    Ok(items(agent, url)?
        .iter()
        .filter_map(|i| i.get("name").and_then(|n| n.as_str()).map(|s| s.to_string()))
        .collect())
}

// ---------------------------------------------------------------- 目录缓存

fn cache_path(kind: &str) -> PathBuf {
    // 两个通道的设备目录各自缓存：售后通道有五百多台设备、只在真要查售后包时
    // 才值得拉，混在一个文件里会拖慢常规查询。
    let name = if kind == KIND_AS {
        "violet-catalog-after.json"
    } else {
        "violet-catalog.json"
    };
    std::env::temp_dir().join("ota-tool").join(name)
}

fn load_cache(path: &Path) -> Option<(u64, Vec<Device>)> {
    let raw = std::fs::read_to_string(path).ok()?;
    let v: Value = serde_json::from_str(&raw).ok()?;
    let ts = v.get("ts").and_then(|x| x.as_u64())?;
    let devs: Vec<Device> = serde_json::from_value(v.get("devices")?.clone()).ok()?;
    if devs.is_empty() {
        None
    } else {
        Some((ts, devs))
    }
}

fn save_cache(path: &Path, devs: &[Device]) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let v = serde_json::json!({ "ts": now_secs(), "devices": devs });
    if let Ok(s) = serde_json::to_string(&v) {
        let _ = std::fs::write(path, s);
    }
}

fn fetch_catalog(kind: &str) -> Result<Vec<Device>, String> {
    let a = agent();
    let mut out = Vec::new();
    let brands = get_strings(&a, &format!("{}/brands?packageType={}", API, kind))?;
    for b in brands {
        // 个别系列是空目录，接口直接回 500，跳过就是 ——
        // 不能让一个坏系列把整张目录拖没。
        let su = format!("{}/series?packageType={}&brand={}", API, kind, enc(&b));
        let series = match get_strings(&a, &su) {
            Ok(v) => v,
            Err(_) => continue,
        };
        for s in series {
            let du = format!(
                "{}/devices?packageType={}&brand={}&series={}",
                API,
                kind,
                enc(&b),
                enc(&s)
            );
            let devices = match get_strings(&a, &du) {
                Ok(v) => v,
                Err(_) => continue,
            };
            for d in devices {
                out.push(Device {
                    brand: b.clone(),
                    series: s.clone(),
                    device: d,
                });
            }
        }
    }
    Ok(out)
}

/// 指定通道的设备目录（磁盘缓存 1 天；连不上时回退到过期缓存）
pub fn catalog_kind(kind: &str) -> Result<Vec<Device>, String> {
    let path = cache_path(kind);
    let cached = load_cache(&path);
    if let Some((ts, devs)) = &cached {
        if now_secs().saturating_sub(*ts) < CACHE_TTL {
            return Ok(devs.clone());
        }
    }
    match fetch_catalog(kind) {
        Ok(v) if !v.is_empty() => {
            save_cache(&path, &v);
            Ok(v)
        }
        Ok(_) => cached
            .map(|(_, d)| d)
            .ok_or_else(|| "社区源返回了空目录".to_string()),
        Err(e) => cached.map(|(_, d)| d).ok_or(e),
    }
}

// ---------------------------------------------------------------- 名称归一

/// 归一化：去空格、统一大小写、全角括号转半角
fn norm(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| match c {
            '（' => '(',
            '）' => ')',
            '－' => '-',
            _ => c.to_ascii_lowercase(),
        })
        .collect()
}

/// 取出设备名里的「型号主体」：去掉前缀标记、品牌词与末尾的型号码
fn core_of(name: &str) -> String {
    let mut s = name.to_string();
    for p in ["[普通]", "[C16动态解析]", "[C16]", "[动态解析]"] {
        s = s.replace(p, "");
    }
    for w in [
        "真我", "一加", "欧加", "OPPO", "Oppo", "oppo", "Realme", "realme", "OnePlus", "oneplus",
    ] {
        s = s.replace(w, "");
    }
    let cut = [s.rfind('('), s.rfind('（')].into_iter().flatten().max();
    if let Some(c) = cut {
        s.truncate(c);
    }
    norm(&s)
}

/// 售后通道设备名的归一化键
///
/// 售后通道的设备名与常规通道完全不同：不带 `[普通]` 前缀、不带型号码括号，
/// 空格与下划线混用（`真我GT_Neo5`、`OPPO_K10_5G`、`一加13`）。常规通道那套
/// `core_of` 在这里会失效，得单独做一套：
///
/// 1. 剥掉 `[普通]` / `[C16]` 这类方括号前缀；
/// 2. 只取斜杠前第一段（`OPPO Reno16 / OPPO Reno16t` 取前者）；
/// 3. 剥掉品牌词；
/// 4. 剥掉全部括号内容（全角、半角都算）；
/// 5. 循环剥掉尾部的修饰词（国行版 / 5g / 定制版 …）；
/// 6. 删掉空白与 `_` `-` `·` 这些连接符；
/// 7. 转小写。
///
/// 实测把这套键与机型表对照，539 台机型里 300 台国行手机能确切命中，
/// 剩下的多是手表、平板与售后库本身就没收录的老机型。
fn as_key(name: &str) -> String {
    let mut s = name.to_string();
    for p in ["[普通]", "[C16动态解析]", "[C16]", "[动态解析]"] {
        s = s.replace(p, "");
    }
    if let Some(first) = s.split('/').next() {
        s = first.to_string();
    }
    for w in BRAND_WORDS {
        s = s.replace(w, "");
    }
    // 剥括号内容：全角与半角，可能出现多组
    let mut out = String::with_capacity(s.len());
    let mut depth = 0i32;
    for c in s.chars() {
        match c {
            '(' | '（' => depth += 1,
            ')' | '）' => depth -= 1,
            _ if depth > 0 => {}
            _ => out.push(c),
        }
    }
    let mut s = out;
    // 尾部修饰词可能叠加（「一加 13 国行版 5G」），循环剥到剥不动为止。
    // 长词先试，否则「定制版」会被「版」先吃掉半截。
    loop {
        let low = s.to_lowercase();
        let mut hit = false;
        for t in OVERSEAS.iter().chain(AS_SUFFIX.iter()) {
            let tl = t.to_lowercase();
            if low.ends_with(&tl) && low.len() > tl.len() {
                s.truncate(s.len() - t.len());
                s = s.trim_end().to_string();
                hit = true;
                break;
            }
        }
        if !hit {
            break;
        }
    }
    s.chars()
        .filter(|c| !c.is_whitespace() && !matches!(c, '_' | '-' | '·' | '・'))
        .collect::<String>()
        .to_lowercase()
}

/// 我们的机型表品牌 → 社区目录品牌名
fn family(brand: &str) -> Option<&'static str> {
    match brand {
        "realme" | "Realme" => Some("Realme"),
        "OnePlus" | "oneplus" => Some("OnePlus"),
        "OPPO" | "Oppo" => Some("OPPO"),
        _ => None,
    }
}

/// 在社区目录里找出该机型的全部设备条目
///
/// 同一台机器在社区目录里常有两套条目（带型号码的普通条目与不带型号码的
/// 动态解析条目），版本清单互补，必须一起取。
pub fn find(code: &str, name: &str, brand: &str) -> Vec<Device> {
    find_kind(code, name, brand, KIND_FULL)
}

/// 在指定通道的设备目录里找出该机型
///
/// 两个通道的设备命名规则完全不同，匹配策略也不一样：
/// - 常规通道按型号码（RMX3888）与型号主体名（gt5pro）匹配，见 `core_of`；
/// - 售后通道的设备名里没有型号码，只能按品牌的型号主体名匹配，见 `as_key`。
pub fn find_kind(code: &str, name: &str, brand: &str, kind: &str) -> Vec<Device> {
    let Ok(all) = catalog_kind(kind) else {
        return Vec::new();
    };
    let fam = family(brand);
    let pool: Vec<&Device> = all
        .iter()
        .filter(|d| match fam {
            Some(f) => d.brand == f,
            None => matches!(d.brand.as_str(), "OnePlus" | "OPPO" | "Realme"),
        })
        .collect();

    let mut picked: Vec<Device> = Vec::new();

    if kind == KIND_AS {
        // 售后通道：设备名里没有型号码，只能按「品牌 + 型号主体名」精确匹配。
        // 用等值而不是包含，是因为这里的名字都很短（`真我GT` / `真我GT5` /
        // `真我GT5_Pro`），用包含会把相邻型号一锅端。
        let target = as_key(name);
        if target.is_empty() {
            return picked;
        }
        for d in &pool {
            if as_key(&d.device) == target {
                picked.push((*d).clone());
            }
        }
        // 同一台机器的定制版条目（`真我GT5_Pro` 与 `真我GT5_Pro竞速版`）名字只差
        // 一个后缀，归一键不同，但对用户是同一台机器：再用前缀兜一层。
        if picked.is_empty() {
            for d in &pool {
                if as_key(&d.device).starts_with(&target) {
                    picked.push((*d).clone());
                }
            }
        }
        return picked;
    }

    let ncode = norm(code);
    let mut cores: HashSet<String> = HashSet::new();

    if ncode.chars().count() >= 4 {
        for d in &pool {
            if norm(&d.device).contains(&ncode) {
                cores.insert(core_of(&d.device));
                picked.push((*d).clone());
            }
        }
    }

    // 按机型名匹配：品牌已知时允许短名（如「一加 13」主体只有 13）
    let target = core_of(name);
    let n = target.chars().count();
    if n > 0 && (n >= 3 || fam.is_some()) {
        cores.insert(target);
    }

    for d in &pool {
        if cores.contains(&core_of(&d.device))
            && !picked
                .iter()
                .any(|p| p.brand == d.brand && p.series == d.series && p.device == d.device)
        {
            picked.push((*d).clone());
        }
    }
    picked
}

// ---------------------------------------------------------------- 版本与直链

/// 社区源用中文词标注包的用途。实测全库只有「降级包」一种写法，
/// 同类词也一并认下：对方哪天换了说法，不至于把降级包当成普通版本混进清单。
const AFTER_MARKS: [&str; 4] = ["降级包", "回退包", "维修包", "售后包"];

/// 版本名里是否带「这是降级包」的标记
fn after_mark(raw: &str) -> bool {
    raw.split_whitespace()
        .any(|t| AFTER_MARKS.iter().any(|m| t == *m))
}

/// 把社区源的版本名拆成「展示名 + 分支字母」
///
/// 实测三种形态：带 RealmeUI 前缀与分支后缀的、纯版本号的、尾部标「降级包」的。
fn split_branch(raw: &str) -> (String, String) {
    let mut s = raw.trim().to_string();
    if let Some(r) = s.strip_prefix("RealmeUI ") {
        s = r.trim().to_string();
    }
    let mut branch = String::new();
    loop {
        let t = s.trim_end().to_string();
        let Some(last) = t.split_whitespace().last().map(|x| x.to_string()) else {
            break;
        };
        let is_branch = {
            let b = last.as_bytes();
            b.len() >= 3
                && b[0].is_ascii_uppercase()
                && b[1] == b'.'
                && b[2..].iter().all(|c| c.is_ascii_digit())
        };
        if is_branch {
            branch = last[..1].to_string();
            s = t[..t.len() - last.len()].trim_end().to_string();
        } else if after_mark(&last) {
            s = t[..t.len() - last.len()].trim_end().to_string();
        } else {
            break;
        }
    }
    (s, branch)
}

/// 从版本号推 Android 大版本
///
/// 实测官方数据里 14.1.0.805 → Android 14、15.0.0.1300 → Android 15、
/// 16.0.10.500 → Android 16，首个数字就是 Android 大版本。
/// 社区源不返回这个字段，只能这样补；范围外一律留空，不瞎猜。
fn android_of(version: &str) -> String {
    let head = match version.split_once('_') {
        Some((_, rest)) => rest,
        None => version,
    };
    let digits: String = head.chars().take_while(|c| c.is_ascii_digit()).collect();
    match digits.parse::<u32>() {
        // 与官方源返回的写法保持一致（官方是 "Android 16"）
        Ok(n) if (12..=20).contains(&n) => format!("Android {}", n),
        _ => String::new(),
    }
}

fn versions(a: &ureq::Agent, d: &Device) -> Vec<String> {
    versions_kind(a, d, KIND_FULL)
}

/// 指定通道的版本名清单（原样，未拆解）
fn versions_kind(a: &ureq::Agent, d: &Device, kind: &str) -> Vec<String> {
    let url = format!(
        "{}/versions?packageType={}&brand={}&series={}&device={}",
        API,
        kind,
        enc(&d.brand),
        enc(&d.series),
        enc(&d.device)
    );
    get_names(a, &url).unwrap_or_default()
}

/// 社区源查到的两类包
///
/// 降级包必须单独收：它的版本号常常是旧的（RMX3888_14.1.0.804(CN01)），
/// 混进普通清单会被当成「一个很老的升级包」，而它真正的身份是「能刷回去的
/// 降级包」，用法完全不同 —— 就像官方售后包那样，必须另立分区。
pub struct Community {
    /// 普通版本清单
    pub normal: Vec<Package>,
    /// 标注为降级 / 回退的包
    pub after: Vec<Package>,
}

/// 查该机型在社区源里的全部版本，拆成普通清单与降级包两部分
pub fn packages(code: &str, name: &str, brand: &str) -> Community {
    let mut out = Community {
        normal: Vec::new(),
        after: Vec::new(),
    };
    let devs = find(code, name, brand);
    if devs.is_empty() {
        return out;
    }
    let a = agent();
    // 两个桶各自去重：同一个版本号可能既有普通包又有降级包，
    // 它们是两个不同的文件，不能互相顶掉。
    let mut seen_normal: HashSet<String> = HashSet::new();
    let mut seen_after: HashSet<String> = HashSet::new();
    for d in &devs {
        for raw in versions(&a, d) {
            let is_after = after_mark(&raw);
            let (disp, branch) = split_branch(&raw);
            if disp.is_empty() {
                continue;
            }
            let dup = if is_after {
                !seen_after.insert(ota::version_key(&disp))
            } else {
                !seen_normal.insert(ota::version_key(&disp))
            };
            if dup {
                continue;
            }
            let pkg = Package {
                android: android_of(&disp),
                version: disp,
                size: 0,
                manual_url: String::new(),
                published: String::new(),
                security_patch: String::new(),
                branch,
                md5: String::new(),
                ota_version: String::new(),
                source: if is_after {
                    Source::AfterCommunity
                } else {
                    Source::Community
                },
                // 换直链必须回传原始版本名（带「降级包」后缀那个），
                // 拆过的展示名它不认。
                vio: Some(Ref {
                    brand: d.brand.clone(),
                    series: d.series.clone(),
                    device: d.device.clone(),
                    version: raw,
                    kind: KIND_FULL.to_string(),
                }),
                link_ok: None,
                note: String::new(),
                pack: meta::Pack::Ab,
            };
            if is_after {
                out.after.push(pkg);
            } else {
                out.normal.push(pkg);
            }
        }
    }
    out
}

/// 售后通道的整个版本清单
///
/// 这个通道是「售后包」专用：一个机型常有三十几条，比常规清单还长，
/// 而且封装分两类 —— 新版是 A/B 全量包，老版是 OFP / EDL 线刷包。
/// 后者必须用 EDL 工具走 9008 端口刷入，和系统更新完全不是一回事。
///
/// 与常规清单**不去重**：同一个版本号在两个通道里是两个不同的文件
/// （实测体积差 2.2~2.6 GB、文件名也不同），合并成一条会让用户下错。
pub fn after_sales(code: &str, name: &str, brand: &str) -> Vec<Package> {
    let devs = find_kind(code, name, brand, KIND_AS);
    if devs.is_empty() {
        return Vec::new();
    }
    let a = agent();
    let mut out: Vec<Package> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for d in &devs {
        for raw in versions_kind(&a, d, KIND_AS) {
            let Some(pkg) = as_package(code, d, &raw) else {
                continue;
            };
            if seen.insert(pkg.key()) {
                out.push(pkg);
            }
        }
    }
    out
}

/// 把售后通道的版本名拆成一个 Package
///
/// 实测两种写法：
/// - `15.0.0.520(CN01) · 稳定版 · 9.37 GB`（多数，体积与补丁日就在名字里）
/// - `RMX3706domestic_11_A.43_2023070817200116.zip`（少数老的 EDL 包，自带代号）
///
/// 中间可能还有一段 `补丁 2025-05-01`。名字里的体积按 1 GB = 1024³ 换算，
/// 实测与真实字节数误差不到 1%，先用它撑住卡片，真值等换完直链再要。
fn as_package(code: &str, d: &Device, raw: &str) -> Option<Package> {
    let mut version = String::new();
    let mut security_patch = String::new();
    let mut size = 0u64;
    for (i, seg) in raw.split('·').map(str::trim).enumerate() {
        if seg.is_empty() {
            continue;
        }
        if i == 0 {
            version = seg.strip_suffix(".zip").unwrap_or(seg).to_string();
            continue;
        }
        if let Some(rest) = seg.strip_prefix("补丁") {
            let p = rest.trim();
            // 年份明显是占位值的（社区库里出现过 2088-xx-xx）不认
            if p.len() == 10 && p.as_bytes()[4] == b'-' {
                if let Ok(y) = p[..4].parse::<u32>() {
                    if (2010..=2030).contains(&y) {
                        security_patch = p.to_string();
                    }
                }
            }
            continue;
        }
        if size == 0 {
            size = parse_size(seg);
        }
    }
    if version.is_empty() {
        return None;
    }
    // 裸版本号（15.0.0.520(CN01)）要补上机型代号，否则卡片上认不出是谁的包；
    // 已经自带代号的（RMX3706domestic_...）不能再补，会变成「RMX3706_RMX3706_」。
    let shown = if version.to_ascii_lowercase().contains(&code.to_ascii_lowercase()) {
        version.clone()
    } else {
        format!("{}_{}", code, version)
    };
    Some(Package {
        android: android_of(&version),
        version: shown,
        size,
        manual_url: String::new(),
        published: String::new(),
        security_patch,
        branch: branch_in(&version),
        md5: String::new(),
        ota_version: String::new(),
        source: Source::AfterCommunity,
        // 换直链必须回传原始名字（带 · 稳定版 · 体积 那一整串），拆过的它不认
        vio: Some(Ref {
            brand: d.brand.clone(),
            series: d.series.clone(),
            device: d.device.clone(),
            version: raw.to_string(),
            kind: KIND_AS.to_string(),
        }),
        link_ok: None,
        note: String::new(),
        pack: meta::Pack::Unknown,
    })
}

/// 名字里的体积段，如 `9.37 GB`
fn parse_size(seg: &str) -> u64 {
    let mut it = seg.split_whitespace();
    let Some(num) = it.next().and_then(|s| s.parse::<f64>().ok()) else {
        return 0;
    };
    let unit = it.next().unwrap_or("GB").to_ascii_uppercase();
    let mul = match unit.as_str() {
        "TB" => 1_099_511_627_776f64,
        "GB" => 1_073_741_824f64,
        "MB" => 1_048_576f64,
        _ => return 0,
    };
    if num <= 0.0 {
        0
    } else {
        (num * mul) as u64
    }
}

/// 版本号里的分支字母：`_11.A.43_` → A
fn branch_in(version: &str) -> String {
    let Some(third) = version.split('_').nth(2) else {
        return String::new();
    };
    let mut cs = third.chars();
    match (cs.next(), cs.next()) {
        (Some(c), Some('.')) if c.is_ascii_uppercase() => c.to_string(),
        _ => String::new(),
    }
}

/// 给售后通道的包补上「真实体积 + 封装形态」
///
/// 名字里那个体积是四舍五入过的，卡片上要显示精确值就得问一次；
/// 顺手还能从 ZIP 首部判出这是 A/B 全量包还是 OFP 线刷包 —— 这一步很要紧，
/// 线刷包丢进系统更新里是刷不进去的，用户必须提前知道。
///
/// 六路并发：31 条实测 8 秒左右跑完，单条 Range 请求只要 4~64 KB。
pub fn enrich_as(list: Vec<Package>) -> Vec<Package> {
    let mut out = Vec::with_capacity(list.len());
    for chunk in list.chunks(6) {
        let done: Vec<Package> = std::thread::scope(|s| {
            let handles: Vec<_> = chunk
                .iter()
                .cloned()
                .map(|mut p| {
                    s.spawn(move || {
                        fill_as(&mut p);
                        p
                    })
                })
                .collect();
            handles
                .into_iter()
                .enumerate()
                .map(|(i, h)| h.join().unwrap_or_else(|_| chunk[i].clone()))
                .collect()
        });
        out.extend(done);
    }
    out
}

/// 单个售后包的补齐：换直链 → 读首部 → 填体积与封装
fn fill_as(pkg: &mut Package) {
    let Some(r) = pkg.vio.clone() else {
        return;
    };
    let Ok(url) = download_link(&r) else {
        pkg.link_ok = Some(false);
        return;
    };
    // 体积来自版本名，先当作可用；读不到首部时它就是最终值。
    let estimated = pkg.size;
    match meta::head(&url) {
        Some(h) => {
            pkg.pack = meta::pack_of(&h.buf);
            if h.size > 0 {
                pkg.size = h.size;
            }
            pkg.link_ok = Some(true);
        }
        None => {
            // 首部读不到只是判不出封装，链接本身多半还是好的：
            // 用一次 Range 探活，能拿到长度就当它可用。
            pkg.size = fetch_size(&url).unwrap_or(estimated);
            pkg.link_ok = Some(pkg.size > 0);
        }
    }
}

/// 给社区降级包补上「体积 / 发布日 / 补丁」这三项它自己不给的信息。
///
/// 社区源的版本接口只有名字，卡片上「大小」一栏会一直写着「未知」，
/// 和官方售后卡摆在一起非常割裂。降级包的地址换出来之后是裸 CDN 直链，
/// 发一次 Range 就能拿到 content-range 里的总大小，顺手还能读到 ZIP 首部的
/// META-INF/com/android/metadata 补齐发布日与补丁日。
///
/// 补不出来就保持原样：这一步只影响显示，绝不能因为它失败就不给下载。
pub fn enrich_after(list: Vec<Package>) -> Vec<Package> {
    let mut out = Vec::with_capacity(list.len());
    // 三路并发而不是全展开：社区源是免费公共服务，一次打几十个请求容易被限流，
    // 单机型通常也就 1~3 个降级包，三路足够把往返时间压到一条链路的量级。
    for chunk in list.chunks(3) {
        let done: Vec<Package> = std::thread::scope(|s| {
            let handles: Vec<_> = chunk
                .iter()
                .cloned()
                .map(|mut p| {
                    s.spawn(move || {
                        fill_after(&mut p);
                        p
                    })
                })
                .collect();
            handles
                .into_iter()
                .enumerate()
                .map(|(i, h)| h.join().unwrap_or_else(|_| chunk[i].clone()))
                .collect()
        });
        out.extend(done);
    }
    out
}

/// 单个社区降级包的补齐：换直链 → 取首部 → 填字段
fn fill_after(pkg: &mut Package) {
    let Some(r) = pkg.vio.clone() else {
        return;
    };
    let Ok(url) = download_link(&r) else {
        // 换不出直链才真的是「链接已失效」；能换出来但读不到首部只是补不全信息
        pkg.link_ok = Some(false);
        return;
    };
    let Some((m, size)) = meta::fetch(&url) else {
        // 首部读不到时退一步：只要 HEAD/Range 能拿到长度，就说明链接还活着
        pkg.size = fetch_size(&url).unwrap_or(pkg.size);
        pkg.link_ok = Some(pkg.size > 0);
        return;
    };
    if size > 0 {
        pkg.size = size;
    }
    if !m.published.is_empty() {
        pkg.published = m.published;
    }
    if !m.security_patch.is_empty() {
        pkg.security_patch = m.security_patch;
    }
    // 版本名与分支沿用社区源自己的写法：社区用点分（RMX3300_11.C.16），
    // 而 ZIP 里记的是下划线分（RMX3300_11_C.16），界面上前者更像官网的写法。
    pkg.link_ok = Some(true);
}

/// 用四元组换一条签名直链（有效期约 10 分钟，不能预存）
pub fn download_link(r: &Ref) -> Result<String, String> {
    // packageType 必须与当初查清单时用的通道一致：两个通道的设备名互不通用，
    // 拿售后通道的设备名去请求 full 一律 404。
    let body = serde_json::json!({
        "packageType": r.kind_of(),
        "brand": r.brand,
        "series": r.series,
        "device": r.device,
        "version": r.version,
    });
    let resp = agent()
        .post(&format!("{}/download-link", API))
        .set("Content-Type", "application/json")
        .send_json(&body)
        .map_err(err_text)?;
    let v: Value = resp.into_json().map_err(|e| e.to_string())?;
    if let Some(u) = v
        .get("links")
        .and_then(|x| x.as_array())
        .and_then(|a| a.first())
        .and_then(|x| x.as_str())
    {
        if !u.trim().is_empty() {
            return Ok(u.to_string());
        }
    }
    let msg = v
        .get("errorMessage")
        .and_then(|x| x.as_str())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("社区源没有返回下载地址");
    Err(msg.to_string())
}

/// 用一次 Range 请求问出文件总大小（顺便验证链接是否还有效）
pub fn fetch_size(url: &str) -> Result<u64, String> {
    let resp = agent()
        .get(url)
        .set("Range", "bytes=0-0")
        .call()
        .map_err(err_text)?;
    if let Some(cr) = resp.header("content-range") {
        if let Some(n) = cr.rsplit('/').next().and_then(|s| s.trim().parse::<u64>().ok()) {
            if n > 0 {
                return Ok(n);
            }
        }
    }
    if let Some(cl) = resp.header("content-length") {
        if let Ok(n) = cl.trim().parse::<u64>() {
            if n > 0 {
                return Ok(n);
            }
        }
    }
    Err("服务器没有返回文件大小".into())
}

/// 探测一个版本的链接是否还能下载：能下返回文件大小，不能下返回 None
pub fn probe(r: &Ref) -> Option<u64> {
    let url = download_link(r).ok()?;
    fetch_size(&url).ok()
}

/// 同步取文件大小（下载前的兜底）
pub fn size_of(r: &Ref) -> Result<u64, String> {
    let url = download_link(r)?;
    fetch_size(&url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_key_strips_code_and_region() {
        assert_eq!(ota::version_key("RMX3888_16.0.10.500(CN01)"), "16.0.10.500");
        assert_eq!(ota::version_key("RMX3888_16.0.10.500（CN01）"), "16.0.10.500");
        assert_eq!(ota::version_key("PJZ110_16.0.9.402(CN01)"), "16.0.9.402");
        assert_eq!(ota::version_key("16.0.10.500"), "16.0.10.500");
    }

    #[test]
    fn version_key_never_empty_for_text() {
        // 没有版本号的纯文本回退成小写原文，保证去重键不塌成空串
        assert_eq!(ota::version_key("降级包"), "降级包");
    }

    #[test]
    fn split_branch_handles_all_three_shapes() {
        assert_eq!(
            split_branch("RealmeUI RMX3888_14.0.0.231(CN01) A.22"),
            ("RMX3888_14.0.0.231(CN01)".to_string(), "A".to_string())
        );
        assert_eq!(
            split_branch("RMX3888_16.0.10.500(CN01)"),
            ("RMX3888_16.0.10.500(CN01)".to_string(), String::new())
        );
        assert_eq!(
            split_branch("RealmeUI RMX3888_14.1.0.804(CN01) 降级包"),
            ("RMX3888_14.1.0.804(CN01)".to_string(), String::new())
        );
        assert_eq!(
            split_branch("RMX3888_15.0.0.1300(CN01) C.77"),
            ("RMX3888_15.0.0.1300(CN01)".to_string(), "C".to_string())
        );
    }

    #[test]
    fn core_of_drops_prefix_brand_and_code() {
        // 全角、半角括号都要吃得住
        assert_eq!(core_of("[普通]真我 Realme GT5 Pro（RMX3888）"), "gt5pro");
        assert_eq!(core_of("[C16动态解析]Realme GT5 Pro"), "gt5pro");
        assert_eq!(core_of("[普通]OPPO Find X8 (PKB110)"), "findx8");
        assert_eq!(core_of("[C16动态解析]OnePlus 13"), "13");
    }

    #[test]
    fn core_of_merges_the_two_entries_of_one_device() {
        // 社区目录里同一台机器常有两套条目，去掉前缀与型号码后必须落成同一个键
        assert_eq!(
            core_of("[普通]真我 Realme GT5 Pro（RMX3888）"),
            core_of("[C16动态解析]Realme GT5 Pro")
        );
        assert_eq!(
            core_of("[普通]OPPO Find X8（PKB110）"),
            core_of("[普通]OPPO Find X8 (PKB110)")
        );
    }

    #[test]
    fn norm_is_case_and_width_insensitive() {
        assert_eq!(norm("GT5 Pro"), "gt5pro");
        assert_eq!(norm("（CN01）"), "(cn01)");
    }

    #[test]
    fn family_maps_our_brands_to_their_names() {
        assert_eq!(family("realme"), Some("Realme"));
        assert_eq!(family("OnePlus"), Some("OnePlus"));
        assert_eq!(family("OPPO"), Some("OPPO"));
        assert_eq!(family("Nubia"), None);
    }

    #[test]
    fn enc_percent_encodes_non_reserved() {
        assert_eq!(enc("Realme"), "Realme");
        assert_eq!(enc("GT系列"), "GT%E7%B3%BB%E5%88%97");
        // 半角括号、空格都要编码，否则 URL 会被截断
        assert_eq!(enc("Find X8 (PKB110)"), "Find%20X8%20%28PKB110%29");
        // 全角括号按 UTF-8 字节编码，不能原样留在 URL 里
        let full = enc("Find X8（PKB110）");
        assert!(!full.contains(' '));
        assert!(!full.contains('（'));
        assert!(full.contains("%EF%BC%88"));
    }
}
