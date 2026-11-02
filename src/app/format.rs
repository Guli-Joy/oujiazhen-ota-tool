//! 数值格式化小工具
/// 字节 → "8.76 GB"
pub fn bytes(n: u64) -> String {
    const G: f64 = 1_073_741_824.0;
    const M: f64 = 1_048_576.0;
    let f = n as f64;
    if f >= G {
        format!("{:.2} GB", f / G)
    } else if f >= M {
        format!("{:.0} MB", f / M)
    } else if n == 0 {
        "未知".to_string()
    } else {
        format!("{} KB", n / 1024)
    }
}

/// unix 毫秒 → "2026-08-13"
pub fn date(ms: i64) -> String {
    if ms <= 0 {
        return "未知".to_string();
    }
    let days = ms / 1000 / 86400;
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    format!("{:04}-{:02}-{:02}", y, m, d)
}

/// 剩余秒数 → "12 分 30 秒"
pub fn dur(secs: i64) -> String {
    if secs <= 0 {
        return "—".to_string();
    }
    if secs >= 3600 {
        format!("{} 时 {} 分", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{} 分 {} 秒", secs / 60, secs % 60)
    } else {
        format!("{} 秒", secs)
    }
}
