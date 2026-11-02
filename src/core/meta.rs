//! 售后全量包的「真实身份」：从 ZIP 首部读 META-INF/com/android/metadata。
//!
//! 为什么需要：降级服务器只下发 colorosVersion（"ColorOS16.1.0" 这种大版本号），
//! 同一机型的所有售后包在这个字段上长得一模一样，界面照着显示等于什么都没说
//! —— 用户看到两张卡都是 "ColorOS16.1.0"，自然觉得「售后不对」。
//!
//! 而 A/B 全量包本身就是个 ZIP：第一个条目固定是 META-INF/com/android/metadata，
//! 未压缩、明文 key=value，里面就写着 version_name / ota_version / security_patch。
//! 发一次 Range 请求取前 64 KB 即可读到（实测 206），代价约等于一次网络往返，
//! 比让界面显示一个毫无区分度的大版本号划算得多。
//!
//! 读不到就当没有：售后包照旧按服务器给的信息显示，绝不让这一步把整次查询拖垮。

use std::io::Read;
use std::time::Duration;

/// 取多少字节。metadata 条目实测都在 1.3 KB 以内，64 KB 足够覆盖
/// 「前面还排着几个大条目」的包；再多就是白费流量。
const PREFIX_LEN: u64 = 64 * 1024;

/// metadata 条目的固定路径
const META_NAME: &str = "META-INF/com/android/metadata";

/// ZIP 本地文件头签名
const SIG: [u8; 4] = [0x50, 0x4b, 0x03, 0x04];

/// 从 metadata 里挑出来的、界面用得上的字段
#[derive(Clone, Default)]
pub struct Meta {
    /// 真正的版本号，如 "RMX3888_16.0.10.500(CN01)"
    pub version: String,
    /// 内部版本号，如 "RMX3888_11.F.35_2350_202608121732"
    pub ota_version: String,
    /// 已格式化成界面文案，如 "Android16"
    pub android: String,
    /// 安全补丁，如 "2026-08-01"
    pub security_patch: String,
    /// 构建时间，unix 毫秒（统一成和官方源一样的单位，方便界面直接 date()）
    pub published: String,
}

/// 读一个售后直链的包信息。任何一步不如预期都返回 None。
pub fn read(url: &str) -> Option<Meta> {
    let buf = fetch_head(url)?;
    let text = find_metadata(&buf)?;
    Some(parse(&text))
}

/// 只取前 64 KB。服务器若忽略 Range 直接回 200 全量，take() 也会在第 64 KB 截断，
/// 不会因为一个 8 GB 的包把内存吃光。
fn fetch_head(url: &str) -> Option<Vec<u8>> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(15))
        .build();
    let resp = agent
        .get(url)
        .set("Range", &format!("bytes=0-{}", PREFIX_LEN - 1))
        .call()
        .ok()?;
    let mut buf = Vec::with_capacity(PREFIX_LEN as usize);
    resp.into_reader()
        .take(PREFIX_LEN)
        .read_to_end(&mut buf)
        .ok()?;
    Some(buf)
}

/// 在前缀里按顺序走一遍本地文件头，找到 metadata 条目并取回它的内容。
fn find_metadata(buf: &[u8]) -> Option<String> {
    let mut pos = 0usize;
    while let Some(i) = find(&buf[pos..], &SIG) {
        let at = pos + i;
        // 本地文件头固定 30 字节：4 签名 + 26 字段（末尾是变长的文件名与扩展区）
        let hdr = buf.get(at + 4..at + 30)?;
        let method = u16::from_le_bytes([hdr[4], hdr[5]]);
        let csize = u32::from_le_bytes([hdr[14], hdr[15], hdr[16], hdr[17]]) as usize;
        let nlen = u16::from_le_bytes([hdr[22], hdr[23]]) as usize;
        let elen = u16::from_le_bytes([hdr[24], hdr[25]]) as usize;

        let name_at = at + 30;
        let name_end = name_at + nlen;
        let name = buf.get(name_at..name_end)?;
        let data_at = name_end + elen;

        // ZIP64 的条目把真实大小挪进 extra 字段，这里没有解析 extra 的能力；
        // 遇到就停手，绝不用 0xFFFFFFFF 当长度去切片（必 panic 或被截断）。
        if csize == u32::MAX as usize {
            return None;
        }

        if name == META_NAME.as_bytes() {
            // 只认 stored：deflate 要额外引入解压依赖，而 OPPO 的包这个条目一直是
            // stored，碰不到就不为它背一个依赖。
            if method != 0 {
                return None;
            }
            // 前缀窗口按 64 KB 切，条目被截断时宁可放弃也不返回半截文本
            let end = data_at.checked_add(csize)?;
            let body = buf.get(data_at..end)?;
            return String::from_utf8(body.to_vec()).ok();
        }
        // 下一段从本条数据之后接着找，避免把 payload.bin 里的假签名当成文件头
        pos = data_at.max(at + 4);
    }
    None
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

/// metadata 是纯文本 key=value（换行分隔），按行切即可，不需要真的反序列化。
fn parse(text: &str) -> Meta {
    let mut m = Meta::default();
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let (k, v) = (k.trim(), v.trim());
        if v.is_empty() {
            continue;
        }
        match k {
            // version_name 与 version_name_show 值通常相同，先到先得
            "version_name" | "version_name_show" if m.version.is_empty() => {
                m.version = v.to_string()
            }
            "ota_version" | "ota-id" if m.ota_version.is_empty() => {
                m.ota_version = v.to_string()
            }
            // 与官方源的展示格式对齐（官方源给的就是 "Android 16"，带空格）
            "android_version" if m.android.is_empty() => {
                m.android = format!("Android {}", v)
            }
            // post-security-patch-level 与 security_patch 等价，前者是 A/B 包的惯例字段
            "security_patch" | "post-security-patch-level" if m.security_patch.is_empty() => {
                m.security_patch = v.to_string()
            }
            // 服务器给的 publishedTime 是「此刻」，每次请求都变；这个才是真正的构建时间
            "post-timestamp" if m.published.is_empty() => {
                if let Ok(secs) = v.parse::<i64>() {
                    if secs > 0 {
                        m.published = (secs * 1000).to_string();
                    }
                }
            }
            _ => {}
        }
    }
    m
}
