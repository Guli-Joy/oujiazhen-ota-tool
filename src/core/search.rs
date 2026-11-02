//! 机型搜索：模糊匹配 + 多关键词 AND + 品牌中文别名 + 稳定排序。
//!
//! 纯逻辑模块，不依赖 egui，只用到 std 与 crate::models::MODELS。
//!
//! 匹配流程：
//! 1. 归一化 query（全角转半角、小写、空白折叠、trim）；
//! 2. 按空白切成 token，**所有 token 都必须命中**（AND 语义）；
//! 3. 按命中方式分档打分，多 token 额外加权；
//! 4. 同分时代号短的优先，再按代号字典序，保证结果稳定可复现。

use crate::models::MODELS;
use std::cmp::Ordering;

/// 命中的主要方式，决定打分档位。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchKind {
    /// 代号完全等于 query。
    CodeExact,
    /// 代号以 query 开头。
    CodePrefix,
    /// 代号包含 query。
    CodeContains,
    /// 机型名以 query 开头。
    NamePrefix,
    /// 机型名包含 query。
    NameContains,
    /// 只有品牌命中（例如搜 "realme" / "一加" / "欧珀"）。
    BrandOnly,
}

/// 一条命中结果。字段全部借用自 MODELS 的 &'static str。
#[derive(Clone, Copy, Debug)]
pub struct Hit {
    pub code: &'static str,
    pub name: &'static str,
    pub brand: &'static str,
    pub score: i32,
    /// 命中方式；界面暂未展示，仅单元测试校验排序优先级时使用。
    #[allow(dead_code)]
    pub kind: MatchKind,
}

/// 按关键词搜索机型，返回按相关度从高到低排序、最多 limit 条的结果。
/// query 为空或全是空白时返回空 Vec。
pub fn search(query: &str, limit: usize) -> Vec<Hit> {
    if limit == 0 {
        return Vec::new();
    }
    let q = normalize(query);
    if q.is_empty() {
        return Vec::new();
    }

    let tokens: Vec<&str> = q.split(' ').filter(|t| !t.is_empty()).collect();
    if tokens.is_empty() {
        return Vec::new();
    }

    // 多 token 时每命中一个 token 额外 +40 的次级加权。
    let extra = (tokens.len() as i32 - 1) * 40;

    let mut hits: Vec<Hit> = Vec::new();

    for &(code, name, brand) in MODELS {
        let code_l = code.to_ascii_lowercase();
        let name_l = name.to_lowercase();

        // 逐 token 做 AND 判定；顺带统计哪些 token 命中了代号 / 机型名。
        let mut code_hits = 0usize;
        let mut name_hits = 0usize;
        let mut content: Vec<&str> = Vec::new();
        let mut all_hit = true;

        for &t in &tokens {
            let in_code = code_l.contains(t);
            let in_name = name_l.contains(t);
            let in_brand = brand_matches(brand, t);

            if !in_code && !in_name && !in_brand {
                all_hit = false;
                break;
            }
            if in_code {
                code_hits += 1;
            }
            if in_name {
                name_hits += 1;
            }
            // 品牌 token 只负责"命中"，不参与代号 / 机型名的档位打分。
            if in_code || in_name {
                content.push(t);
            }
        }

        if !all_hit {
            continue;
        }

        let (base, kind) = score_of(&code_l, &name_l, &content, code_hits, name_hits);
        let bonus = match kind {
            MatchKind::NamePrefix | MatchKind::NameContains => coverage_bonus(&q, &name_l),
            _ => 0,
        };

        hits.push(Hit {
            code,
            name,
            brand,
            score: base + extra + bonus,
            kind,
        });
    }

    // 稳定排序：分数降序 → 代号短优先 → 代号字典序。
    hits.sort_by(rank);

    // 按代号去重，取前 limit 条。
    let mut out: Vec<Hit> = Vec::with_capacity(hits.len().min(limit));
    let mut seen: Vec<&'static str> = Vec::new();
    for h in hits {
        if seen.contains(&h.code) {
            continue;
        }
        seen.push(h.code);
        out.push(h);
        if out.len() == limit {
            break;
        }
    }
    out
}

/// 判断输入是否像一个合法的机型代号（用于"直接查代号"分支）：
/// 长度 4..=16，全部是 ASCII 字母或数字（前后空白会被忽略）。
pub fn is_valid_code(s: &str) -> bool {
    let t = s.trim();
    (4..=16).contains(&t.len()) && t.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// 把用户输入规范成大写代号（去空白、全角转半角），供 is_valid_code 之后使用。
pub fn normalize_code(s: &str) -> String {
    s.chars()
        .map(fold_char)
        .filter(|c| !c.is_whitespace())
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

// ---------- 内部实现 ----------

/// 全角转半角：U+3000 → 空格，U+FF01..=U+FF5E → 对应 ASCII。
fn fold_char(c: char) -> char {
    match c {
        '\u{3000}' => ' ',
        '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
        _ => c,
    }
}

/// 归一化：全角转半角 → 小写 → 空白折叠 → trim。
fn normalize(s: &str) -> String {
    let folded: String = s.chars().map(fold_char).collect();
    folded
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// 品牌别名：让用户能用中文搜品牌。品牌值只有 "OPPO" / "realme" / "OnePlus"。
fn brand_zh(brand: &str) -> &'static str {
    match brand {
        "realme" => "真我",
        "OnePlus" => "一加",
        "OPPO" => "欧珀",
        _ => "",
    }
}

/// token 是否命中了品牌（英文品牌名忽略大小写，或命中中文别名）。
fn brand_matches(brand: &str, token: &str) -> bool {
    if brand.eq_ignore_ascii_case(token) {
        return true;
    }
    let zh = brand_zh(brand);
    !zh.is_empty() && zh.contains(token)
}

/// 覆盖度加成：query 在机型名里占的比例越大，越可能是用户想要的那一条。
/// 用于让精确的 "真我 GT5" 排在 "真我 GT5 Pro 阿斯顿马丁 F1 限量版" 前面。
fn coverage_bonus(q: &str, name_l: &str) -> i32 {
    let q_len = q.chars().count();
    let n_len = name_l.chars().count().max(1);
    let raw = (40 * q_len) / n_len;
    raw.min(40) as i32
}

/// 没有内容 token（只命中品牌）时的档位。
const BASE_BRAND_ONLY: i32 = 100;

/// 按命中方式分档打分。
fn score_of(
    code_l: &str,
    name_l: &str,
    content: &[&str],
    code_hits: usize,
    name_hits: usize,
) -> (i32, MatchKind) {
    if content.is_empty() {
        return (BASE_BRAND_ONLY, MatchKind::BrandOnly);
    }

    if content.len() == 1 {
        let t = content[0];
        return if code_l == t {
            (1000, MatchKind::CodeExact)
        } else if code_l.starts_with(t) {
            (800, MatchKind::CodePrefix)
        } else if code_l.contains(t) {
            (600, MatchKind::CodeContains)
        } else if name_l.starts_with(t) {
            (500, MatchKind::NamePrefix)
        } else if name_l.contains(t) {
            (350, MatchKind::NameContains)
        } else {
            (BASE_BRAND_ONLY, MatchKind::BrandOnly)
        };
    }

    // 多 token：先把内容 token 拼成紧凑串，允许 "rmx 3888" 命中 "RMX3888"。
    let mut compact = String::new();
    for &t in content {
        compact.push_str(t);
    }
    let all_on_code = code_hits == content.len();
    let all_on_name = name_hits == content.len();

    if code_l == compact.as_str() {
        (1000, MatchKind::CodeExact)
    } else if all_on_code && code_l.starts_with(content[0]) {
        (800, MatchKind::CodePrefix)
    } else if all_on_code {
        (600, MatchKind::CodeContains)
    } else if name_l.starts_with(compact.as_str()) {
        (500, MatchKind::NamePrefix)
    } else if name_l.starts_with(content[0]) {
        (400, MatchKind::NamePrefix)
    } else if name_l.contains(compact.as_str()) {
        (350, MatchKind::NameContains)
    } else if all_on_name {
        (320, MatchKind::NameContains)
    } else {
        (BASE_BRAND_ONLY, MatchKind::BrandOnly)
    }
}

/// 排序键：分数降序 → 代号长度升序 → 代号字典序升序。
fn rank(a: &Hit, b: &Hit) -> Ordering {
    b.score
        .cmp(&a.score)
        .then_with(|| a.code.len().cmp(&b.code.len()))
        .then_with(|| a.code.cmp(b.code))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 结果里必须存在指定代号。
    fn contains(items: &[Hit], code: &str) -> bool {
        items.iter().any(|h| h.code == code)
    }

    /// 结果里第一个匹配该代号的索引。
    fn pos(items: &[Hit], code: &str) -> Option<usize> {
        items.iter().position(|h| h.code == code)
    }

    #[test]
    fn gt5_pro_hits_rmx3888() {
        // 真实表里 RMX3888 的市场名是 "真我 GT5 Pro"；不断言全名，只断言代号。
        let items = search("GT5 Pro", 10);
        assert!(!items.is_empty(), "\"GT5 Pro\" 应该能搜到结果");
        assert!(contains(&items, "RMX3888"), "\"GT5 Pro\" 应命中 RMX3888");
        assert_eq!(pos(&items, "RMX3888"), Some(0), "RMX3888 应排第一");
        assert_eq!(items[0].brand, "realme");
    }

    #[test]
    fn zhenwo_hits_only_realme() {
        let items = search("真我", 500);
        assert!(!items.is_empty(), "\"真我\" 应命中 realme 机型");
        assert!(
            items.iter().all(|h| h.brand == "realme"),
            "\"真我\" 的结果必须全是 realme 品牌"
        );
    }

    #[test]
    fn find_x8_ranks_plain_before_ultra() {
        let items = search("find x8", 20);
        assert!(items.len() >= 2, "\"find x8\" 应命中多条");
        let plain = pos(&items, "PKB110").expect("应命中 OPPO Find X8");
        let ultra = pos(&items, "PKJ110").expect("应命中 OPPO Find X8 Ultra");
        assert!(plain < ultra, "Find X8 应排在 Find X8 Ultra 之前");
        // Find X8 Pro 系列也要出现。
        assert!(contains(&items, "PKC110"), "应命中 OPPO Find X8 Pro");
    }

    #[test]
    fn code_exact_is_case_insensitive() {
        let items = search("rmx3888", 5);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].code, "RMX3888");
        assert_eq!(items[0].kind, MatchKind::CodeExact);
        assert!(items[0].score >= 1000);

        // 输入全大写同样成立。
        let upper = search("RMX3888", 5);
        assert_eq!(upper[0].code, "RMX3888");
        assert_eq!(upper[0].kind, MatchKind::CodeExact);
    }

    #[test]
    fn empty_query_returns_empty() {
        assert!(search("", 10).is_empty());
        assert!(search("   ", 10).is_empty());
        assert!(search("\t\n ", 10).is_empty());
        assert!(search("\u{3000}\u{3000}", 10).is_empty(), "全角空格也算空白");
        assert!(search("真我", 0).is_empty(), "limit 为 0 时返回空");
    }

    #[test]
    fn is_valid_code_rules() {
        assert!(is_valid_code("RMX3888"));
        assert!(is_valid_code("rmx3888"));
        assert!(is_valid_code("AC2001"));
        assert!(is_valid_code(" OPPO123 "), "前后空白应被忽略");

        assert!(!is_valid_code("RMX"), "只有 3 位，太短");
        assert!(!is_valid_code("RMX3888!!!"), "含非字母数字字符");
        assert!(!is_valid_code("rmx 3888"), "中间不允许空白");
        assert!(!is_valid_code(""));
        assert!(!is_valid_code("12345678901234567"), "超过 16 位");
    }

    #[test]
    fn normalize_code_strips_and_upcases() {
        assert_eq!(normalize_code("rmx3888"), "RMX3888");
        assert_eq!(normalize_code("  rmx 3888  "), "RMX3888");
        assert!(normalize_code("Rmx-3888").contains('-'), "其它字符保留");
        assert_eq!(normalize_code(""), "");
    }

    #[test]
    fn multi_token_uses_and_semantics() {
        // "真我 gt5" 两个 token 必须同时命中。
        let items = search("真我 gt5", 20);
        assert!(!items.is_empty());
        assert!(contains(&items, "RMX3888"), "应命中 真我 GT5 Pro");
        assert!(contains(&items, "RMX3820"), "应命中 真我 GT5");

        // 任一 token 不命中，整条被排除。
        assert!(search("真我 zzz", 20).is_empty());
        assert!(search("gt5 zzzz", 20).is_empty());

        // 代号被空格拆开也能命中。
        let split = search("rmx 3888", 5);
        assert_eq!(split[0].code, "RMX3888");
        assert_eq!(split[0].kind, MatchKind::CodeExact);
    }

    #[test]
    fn brand_alias_queries() {
        let oneplus = search("一加", 500);
        assert!(!oneplus.is_empty(), "\"一加\" 应命中 OnePlus");
        assert!(oneplus.iter().all(|h| h.brand == "OnePlus"));

        let oppo = search("欧珀", 500);
        assert!(!oppo.is_empty(), "\"欧珀\" 应命中 OPPO");
        assert!(oppo.iter().all(|h| h.brand == "OPPO"));
        assert_eq!(oppo[0].kind, MatchKind::BrandOnly);

        let realme = search("realme", 500);
        assert!(!realme.is_empty());
        assert!(realme.iter().all(|h| h.brand == "realme"));

        // 英文品牌名大小写不敏感。
        let oppo_lower = search("oppo", 500);
        assert!(!oppo_lower.is_empty());
        assert!(oppo_lower.iter().all(|h| h.brand == "OPPO"));
    }

    #[test]
    fn fullwidth_query_is_normalized() {
        let items = search("ＲＭＸ３８８８", 5);
        assert_eq!(items[0].code, "RMX3888");
        assert_eq!(items[0].kind, MatchKind::CodeExact);
        // 全角空格当分隔符。
        assert!(!search("真我\u{3000}gt5", 20).is_empty());
    }

    #[test]
    fn results_are_sorted_deduped_and_limited() {
        let all = search("真我", 500);
        // 分数单调不增。
        for w in all.windows(2) {
            assert!(w[0].score >= w[1].score, "结果必须按分数降序");
        }
        // 同分时代号短的优先。
        for w in all.windows(2) {
            if w[0].score == w[1].score {
                assert!(w[0].code.len() <= w[1].code.len(), "同分时代号短的优先");
            }
        }
        // 代号不重复。
        let mut codes: Vec<&str> = all.iter().map(|h| h.code).collect();
        let total = codes.len();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), total, "同一代号不能重复出现");

        // limit 生效。
        assert_eq!(search("真我", 5).len(), 5);
        assert_eq!(search("真我", 1).len(), 1);
    }

    #[test]
    fn hits_are_real_rows_from_models() {
        // 返回值必须真的来自 MODELS，不能凭空造数据。
        let all = search("gt", 50);
        assert!(!all.is_empty());
        for h in &all {
            assert!(
                crate::models::MODELS
                    .iter()
                    .any(|m| m.0 == h.code && m.1 == h.name && m.2 == h.brand),
                "{} 不在 MODELS 表里",
                h.code
            );
        }
    }

    #[test]
    fn no_match_returns_empty() {
        assert!(search("zzzzzz", 10).is_empty());
        assert!(search("这台手机不存在", 10).is_empty());
    }
}
