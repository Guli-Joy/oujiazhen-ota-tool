// OTA 协议层：请求加密、响应解密、直链解析
use aes::Aes256;
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use ctr::cipher::{KeyIvInit, StreamCipher};
use rand::RngCore;
use rsa::{pkcs8::DecodePublicKey, Oaep, RsaPublicKey};
use serde_json::{json, Value};
use sha1::Sha1;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

type Aes256Ctr = ctr::Ctr128BE<Aes256>;

const HOST: &str = "https://component-ota-cn.allawntech.com";
const PUBKEY_B64: &str = "MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEApXYGXQpNL7gmMzzvajHaoZIHQQvBc2cOEhJc7/tsaO4sT0unoQnwQKfNQCuv7qC1Nu32eCLuewe9LSYhDXr9KSBWjOcCFXVXteLO9WCaAh5hwnUoP/5/Wz0jJwBA+yqs3AaGLA9wJ0+B2lB1vLE4FZNE7exUfwUc03fJxHG9nCLKjIZlrnAAHjRCd8mpnADwfkCEIPIGhnwq7pdkbamZcoZfZud1+fPsELviB9u447C6bKnTU4AaMcR9Y2/uI6TJUTcgyCp+ilgU0JxemrSIPFk3jbCbzamQ6Shkw/jDRzYoXpBRg/2QDkbq+j3ljInu0RHDfOeXf3VBfHSnQ66HCwIDAQAB";

/// 版本清单的来源
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug, serde::Serialize, serde::Deserialize)]
pub enum Source {
    /// OPPO / realme / 一加 官方 OTA 服务器
    #[default]
    Official,
    /// 社区源 VioletToolBox 收录的清单（作者 Smart-Paocai，GPL-3.0）
    Community,
    /// 售后（降级 / 维修）包：独立的 ColorOS 降级服务器
    AfterSale,
}

impl Source {
    /// 卡片上的来源角标文案
    pub fn label(self) -> &'static str {
        match self {
            Source::Official => "官方",
            Source::Community => "社区",
            Source::AfterSale => "售后",
        }
    }

    /// 存档用的稳定键（历史记录里按它区分来源）
    pub fn key(self) -> &'static str {
        match self {
            Source::Official => "official",
            Source::Community => "community",
            Source::AfterSale => "aftersale",
        }
    }

    /// 悬停说明：社区来源必须写清作者与项目，避免版权争议
    pub fn tip(self) -> &'static str {
        match self {
            Source::Official => "版本与直链均由 OPPO 官方 OTA 服务器返回",
            Source::Community => "版本清单来自 VioletToolBox（作者 Smart-Paocai，GNU GPL v3.0）\nhttps://github.com/Smart-Paocai/VioletToolBox\n本工具仅调用其公开接口，未复制其源代码",
            Source::AfterSale => "售后（降级 / 维修）全量包，由 ColorOS 官方降级服务器签发\ndowngrade.coloros.com\n用于系统降级或售后维修刷机，直链长期有效、支持断点续传",
        }
    }
}

/// 一个可下载的全量包
#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Package {
    pub version: String,
    pub android: String,
    pub size: u64,
    pub manual_url: String,
    pub published: String,
    pub security_patch: String,
    pub branch: String,
    pub md5: String,
    /// 服务器返回的内部版本号（模型_11.F.xx_yyyymmddhhmm），仅用于排查问题。
    #[allow(dead_code)]
    pub ota_version: String,
    /// 这条版本记录来自哪个源
    pub source: Source,
    /// 社区源换取直链所需的四元组；官方包为 None
    pub vio: Option<crate::core::violet::Ref>,
    /// 社区源链接可用性：None 未探测，Some(true) 可下，Some(false) 已失效
    pub link_ok: Option<bool>,
    /// 备注：售后包用它显示服务器给的用途说明（如「Enzo降级包」）
    pub note: String,
}

impl Package {
    /// 归一化的版本键，用于官方与社区两个源之间去重
    pub fn key(&self) -> String {
        version_key(&self.version)
    }

    /// 稳定身份：同一台机器的同一个包，任何时候都算出同一个值。
    ///
    /// 不能只用版本号：售后包的版本号是「ColorOS16.1.0」这种，同样的字符串会
    /// 出现在一堆不同机型上，用它判断「这张卡是不是正在下载」会串台。
    /// 内部版本号（ota_version）由服务器给出、逐包唯一，拿它做最终的区分依据。
    pub fn id(&self) -> String {
        format!("{}|{}|{}", self.source.key(), self.version, self.ota_version)
    }
}

/// 比较两个版本号的大小：按点的每一段当整数比，段数不同时短的更小。
pub fn version_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let ka = version_key(a);
    let kb = version_key(b);
    let sa: Vec<u64> = ka.split('.').filter_map(|s| s.parse().ok()).collect();
    let sb: Vec<u64> = kb.split('.').filter_map(|s| s.parse().ok()).collect();
    sa.cmp(&sb)
}

/// 把版本号归一成可比较的键：只保留尾部数字与点，去掉机型前缀与地区后缀。
///
/// 两个源写法不一致（例如带不带 (CN01) 后缀），必须归一后才能判断是不是同一版。
pub fn version_key(v: &str) -> String {
    let core = v.split('(').next().unwrap_or(v);
    let core = core.split('（').next().unwrap_or(core);
    let digits: String = core
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    if digits.trim_matches('.').is_empty() {
        core.trim().to_ascii_lowercase()
    } else {
        digits.trim_matches('.').to_string()
    }
}


fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn rand_alnum(n: usize) -> String {
    const CH: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut rng = rand::thread_rng();
    let mut s = String::with_capacity(n);
    for _ in 0..n {
        s.push(CH[(rng.next_u32() as usize) % CH.len()] as char);
    }
    s
}

fn aes_ctr(data: &[u8], key: &[u8], iv: &[u8]) -> Vec<u8> {
    let mut cipher = Aes256Ctr::new(key.into(), iv.into());
    let mut buf = data.to_vec();
    cipher.apply_keystream(&mut buf);
    buf
}

fn wrap_key(aes_key: &[u8]) -> Result<String, String> {
    let der = B64.decode(PUBKEY_B64).map_err(|e| e.to_string())?;
    let pubkey = RsaPublicKey::from_public_key_der(&der).map_err(|e| e.to_string())?;
    let mut rng = rand::thread_rng();
    let plain = B64.encode(aes_key);
    let enc = pubkey
        .encrypt(&mut rng, Oaep::new::<Sha1>(), plain.as_bytes())
        .map_err(|e| e.to_string())?;
    Ok(B64.encode(enc))
}

/// 单次查询：ota_version 形如 RMX3888_11.F.01_0001_197001010000
pub fn query(model: &str, ota_version: &str) -> Result<Package, String> {
    package_of(&query_json(model, ota_version)?)
}

/// 从已解密的响应里取出这个分支的包。
///
/// 和 query_json 分开，是因为分支扫描既要用响应里的包、又要顺手从同一个响应里
/// 捞项目号：同一次网络往返能拿到两样东西，没必要为此多查一遍。
fn package_of(data: &Value) -> Result<Package, String> {
    let comps = data
        .get("components")
        .and_then(|v| v.as_array())
        .ok_or("响应中没有 components")?;
    let first = comps.first().ok_or("components 为空")?;
    let packets = first.get("componentPackets").ok_or("缺少 componentPackets")?;

    fn as_u64(v: Option<&Value>) -> u64 {
        match v {
            Some(Value::Number(n)) => n.as_u64().unwrap_or(0),
            Some(Value::String(s)) => s.parse::<u64>().unwrap_or(0),
            _ => 0,
        }
    }
    fn as_str_or(v: Option<&Value>, d: &str) -> String {
        match v {
            Some(Value::String(s)) if !s.is_empty() => s.clone(),
            Some(Value::Number(n)) => n.to_string(),
            _ => d.to_string(),
        }
    }

    let version = data
        .get("realVersionName")
        .or_else(|| data.get("versionName"))
        .and_then(|v| v.as_str())
        .unwrap_or("-")
        .to_string();

    Ok(Package {
        version,
        android: data
            .get("realAndroidVersion")
            .or_else(|| data.get("androidVersion"))
            .and_then(|v| v.as_str())
            .unwrap_or("-")
            .to_string(),
        size: as_u64(packets.get("size")).max(as_u64(packets.get("sizeV2"))),
        manual_url: as_str_or(packets.get("manualUrl"), ""),
        published: as_str_or(data.get("publishedTime"), ""),
        security_patch: as_str_or(data.get("securityPatch"), "-"),
        branch: String::new(),
        md5: as_str_or(packets.get("md5"), ""),
        ota_version: as_str_or(data.get("otaVersion"), ""),
        source: Source::Official,
        vio: None,
        link_ok: None,
        note: String::new(),
    })
}

/// 原始查询：返回解密后的完整 JSON，便于调试与扩展字段
pub fn query_json(model: &str, ota_version: &str) -> Result<Value, String> {
    let mut aes_key = [0u8; 32];
    let mut iv = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut aes_key);
    rand::thread_rng().fill_bytes(&mut iv);

    let protected = wrap_key(&aes_key)?;
    let pk_json = json!({
        "SCENE_1": {
            "protectedKey": protected,
            "version": (now_ms() as u128 * 1_000_000u128 + 86_400_000_000_000_000u128).to_string(),
            "negotiationVersion": "1615879139745"
        }
    })
    .to_string();

    let body = json!({
        "mode": "0",
        "time": now_ms(),
        "isRooted": "0",
        "isLocked": true,
        "type": "0",
        "deviceId": "0".repeat(64),
        "opex": { "check": true },
        "isSuportPki": false
    })
    .to_string();

    let cipher = aes_ctr(body.as_bytes(), &aes_key, &iv);
    let payload = json!({
        "params": json!({
            "cipher": B64.encode(&cipher),
            "iv": B64.encode(iv)
        })
        .to_string()
    })
    .to_string();

    let agent = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(15))
        .timeout_read(std::time::Duration::from_secs(30))
        .build();

    let resp = agent
        .post(&format!("{}/update/v6", HOST))
        .set("language", "zh-CN")
        .set("newLanguage", "zh-CN")
        .set("androidVersion", "unknown")
        .set("colorOSVersion", "unknown")
        .set("romVersion", "unknown")
        .set("infVersion", "1")
        .set("otaVersion", ota_version)
        .set("model", model)
        .set("mode", "manual")
        .set("nvCarrier", "10010111")
        .set("pipelineKey", "ALLNET")
        .set("operator", "ALLNET")
        .set("companyId", "")
        .set("version", "2")
        .set("deviceId", &rand_alnum(64))
        .set("Content-Type", "application/json; charset=utf-8")
        .set("protectedKey", &pk_json)
        .send_string(&payload);

    let text = match resp {
        Ok(r) => r.into_string().map_err(|e| e.to_string())?,
        Err(ureq::Error::Status(_, r)) => r.into_string().map_err(|e| e.to_string())?,
        Err(e) => return Err(format!("网络错误: {}", e)),
    };

    let root: Value = serde_json::from_str(&text).map_err(|e| format!("响应解析失败: {}", e))?;

    let code = root.get("responseCode").and_then(|v| v.as_i64()).unwrap_or(-1);
    let body_str = match root.get("body").and_then(|v| v.as_str()) {
        Some(s) => s.to_string(),
        None => {
            let msg = root
                .get("errMsg")
                .and_then(|v| v.as_str())
                .unwrap_or("服务器未返回内容");
            return Err(match code {
                2004 => "该机型或该分支没有可用的全量包".to_string(),
                2100 => "参数不符合服务器要求".to_string(),
                _ => format!("查询失败 ({}): {}", code, msg),
            });
        }
    };

    let env: Value = serde_json::from_str(&body_str).map_err(|e| e.to_string())?;
    let c = env.get("cipher").and_then(|v| v.as_str()).ok_or("缺少 cipher")?;
    let i = env.get("iv").and_then(|v| v.as_str()).ok_or("缺少 iv")?;
    let cipher_bytes = B64.decode(c).map_err(|e| e.to_string())?;
    let iv_bytes = B64.decode(i).map_err(|e| e.to_string())?;
    let plain = aes_ctr(&cipher_bytes, &aes_key, &iv_bytes);
    let data: Value = serde_json::from_slice(&plain).map_err(|e| format!("解密失败: {}", e))?;

    Ok(data)
}

/// 项目号的查询结论
///
/// 「查不到」和「没查通」必须分开：前者是这台机器确实没有售后包（可以明确告诉
/// 用户），后者只是这一次没问到（应该提示重试），混成一个 None 就再也解释不清
/// 界面上为什么没有售后分区了。
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Soft {
    /// 拿到了项目号
    Found(String),
    /// 接口正常回话，但响应里没有 `oplus_separate_soft` 字段
    Absent,
    /// 三轮都没拿到正常响应（服务器降级 / 网络异常），结论未知
    Unknown,
}

/// 项目号的内存缓存（只在本次运行内有效）。
///
/// 官方接口对同一机型的连续查询很容易降级，而项目号一旦查到就不会变，
/// 缓存下来至少让「重新扫描」「切回上一个机型」这些操作不再赌运气。
fn soft_cache() -> &'static Mutex<HashMap<String, Soft>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Soft>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cache_get(model: &str) -> Option<Soft> {
    soft_cache().lock().ok()?.get(model).cloned()
}

/// 只缓存「拿到了项目号」这一种。
///
/// 「没有」和「没查通」都不写缓存：它们要么是官方数据本来就缺，要么是这一轮
/// 服务器状态不好，反正值不值得信取决于下一次请求的结果 —— 记住坏结论只会在
/// 用户重扫时把一个可能已经变好的答案继续挡住。
fn cache_put(model: &str, soft: &Soft) {
    if !matches!(soft, Soft::Found(_)) {
        return;
    }
    if let Ok(mut m) = soft_cache().lock() {
        m.insert(model.to_string(), soft.clone());
    }
}

/// 取机型在官方 OTA 里的 `oplus_separate_soft`（5 位项目号）。
///
/// 售后（降级）服务器只认这个项目号，不认机型代号，所以它是两条协议之间
/// 唯一的桥。这个值放在 OTA 响应的 `vabInfo.data.header` 字符串数组里，
/// 形如 `oplus_separate_soft=23607`，与分支无关（同一机型任何分支查出同一个值），
/// 因此这里用通配版本号随便挑几个分支试，拿到一个就返回。
pub fn separate_soft(model: &str) -> Soft {
    // 本次运行里查过一次就够了：项目号是机型的固有属性，不会变。
    if let Some(hit) = cache_get(model) {
        return hit;
    }
    let soft = separate_soft_uncached(model);
    cache_put(model, &soft);
    soft
}

fn separate_soft_uncached(model: &str) -> Soft {
    // 不能只跑一轮。界面查询时这条线和 26 分支扫描、升级链、社区源是同时在跑的，
    // 官方接口会把这种并发判成异常，回一个「没有 components」的降级响应；只跑一轮
    // 很容易刚好撞上，六个分支全被打回就直接 None，整个售后分区凭空消失 —— 这正是
    // 「只有某个机型有售后包」的成因。
    //
    // 要不要再来一轮，看这一轮有没有「拿到完整响应」：
    // - query_json 报错（服务器忙 / 500 / 参数字段不符）：等于没查，必须再来；
    // - 响应里没有 components 或没有 vabInfo.data.header：同样是降级响应、等于没查，
    //   不能据此断定「这台机器没有售后包」。
    // 只有真的读到 header 数组却没找到 oplus_separate_soft，才算查有结论、可以收工。
    for round in 0..3 {
        let mut conclusive = false;
        for letter in ["A", "F", "C", "B", "E", "D"] {
            let ov = format!("{}_11.{}.01_0001_197001010000", model, letter);
            let Ok(v) = query_json(model, &ov) else { continue };
            if let Some(soft) = soft_in(&v) {
                return Soft::Found(soft);
            }
            if responded(&v) {
                conclusive = true;
            }
        }
        if conclusive {
            return Soft::Absent;
        }
        std::thread::sleep(Duration::from_millis(250 * (round + 1) as u64));
    }
    Soft::Unknown
}

/// 探测某机型所有真实存在的分支，返回每个分支的最新包，并顺手从这 26 次响应里
/// 把项目号捞出来。
///
/// 分支扫描本来就要把 A–Z 各问一遍，而这些响应里就带着 `oplus_separate_soft`；
/// 售后通道原本还要为此单独再问一轮官方 OTA，等于把同一批请求打了两遍 —— 官方
/// 接口本来就容易降级，多打一轮就多一分查不到项目号的机会。所以把两件事合成
/// 一件：扫到项目号顺带返回，售后那条线直接用它，不再自己发起查询。
pub fn query_all_branches_harvest(model: &str) -> (Vec<Package>, Soft) {
    let letters: Vec<char> = ('A'..='Z').collect();
    let results = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    // 项目号与分支无关，谁先捞到算谁的；用 OnceLock 语义避免重复回填
    let found: Arc<OnceLock<String>> = Arc::new(OnceLock::new());
    // 有没有任何一个分支真的回了一个包（用于区分「没查到」与「没查通」）。
    // 注意不能看 vabInfo：老机型的正常响应里也没有这个字段，那恰恰是「没有项目号」。
    let conclusive = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let chunk = 6usize;
    for group in letters.chunks(chunk) {
        let mut handles = Vec::new();
        for &letter in group {
            let model_s = model.to_string();
            let slot = results.clone();
            let found_slot = found.clone();
            let conc_slot = conclusive.clone();
            handles.push(std::thread::spawn(move || {
                let ov = format!("{}_11.{}.01_0001_197001010000", model_s, letter);
                let Ok(data) = query_json(&model_s, &ov) else { return };
                if let Some(v) = soft_in(&data) {
                    let _ = found_slot.set(v);
                } else if responded(&data) {
                    conc_slot.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                let Ok(mut p) = package_of(&data) else { return };
                if !p.manual_url.is_empty() {
                    p.branch = letter.to_string();
                    if let Ok(mut v) = slot.lock() {
                        v.push(p);
                    }
                }
            }));
        }
        for h in handles {
            let _ = h.join();
        }
    }
    let soft = match found.get() {
        Some(s) => Soft::Found(s.clone()),
        // 有分支正常回包且确实没有这个字段，才算「官方没给这台机器项目号」
        // （老机型的响应里根本没有 vabInfo，那是正常回话，只是没有项目号）；
        // 26 个分支全都没回包，说明这一轮官方接口一直没正经回话，
        // 那是「没查通」，不能当成「没有」。
        None if conclusive.load(std::sync::atomic::Ordering::Relaxed) => Soft::Absent,
        None => Soft::Unknown,
    };
    // 只有找到项目号才缓存；「没有」和「没查通」都留待下次重问。
    cache_put(model, &soft);
    let mut out = match results.lock() {
        Ok(v) => v.clone(),
        Err(_) => Vec::new(),
    };
    out.sort_by(|a, b| a.branch.cmp(&b.branch));
    (out, soft)
}

/// 这次响应算不算「服务器正常回话」。
///
/// 判据是 components 里有没有真包，而不是有没有 vabInfo：老机型（如 RMX1901）
/// 正常响应里也压根没有 vabInfo 字段 —— 那正好说明官方没给它项目号，属于
/// 「查有结论的没有」；只有连 components 都空掉的那种降级响应才算没查通。
fn responded(data: &Value) -> bool {
    data.get("components")
        .and_then(|c| c.as_array())
        .map(|a| !a.is_empty())
        .unwrap_or(false)
}

/// 取出响应里的 vabInfo.data.header 数组；没有就返回 None
fn header_of(data: &Value) -> Option<&Vec<Value>> {
    data.get("components")?
        .as_array()?
        .first()?
        .get("componentPackets")?
        .get("vabInfo")?
        .get("data")?
        .get("header")?
        .as_array()
}

/// 从一次 OTA 响应里取 `oplus_separate_soft`（5 位项目号）
fn soft_in(data: &Value) -> Option<String> {
    let header = header_of(data)?;
    for item in header {
        let Some(s) = item.as_str() else { continue };
        if let Some(v) = s.strip_prefix("oplus_separate_soft=") {
            let v = v.trim();
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// 顺着升级链一路往前走，把中间经过的版本都收下来
///
/// 官方的 otaVersion 是「起点」而不是精确版本：把响应里的 realOtaVersion 原样
/// 送回服务器，就能拿到它的下一个版本。A.01 是链头，从它出发能走完整个历史，
/// 但这些中间版本恰恰是 A–Z 分支扫描拿不到的（扫描只给每个分支的最新版）。
pub fn query_chain(model: &str) -> Vec<Package> {
    use std::collections::HashSet;
    let mut out: Vec<Package> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut ov = format!("{}_11.A.01_0001_197001010000", model);
    for _ in 0..40 {
        let Ok(mut p) = query(model, &ov) else { break };
        if p.manual_url.is_empty() {
            break;
        }
        // 内部版本号形如 RMX3888_11.C.40_1400_202411111148，中间的字母就是分支
        p.branch = p
            .ota_version
            .split_once('_')
            .and_then(|(_, rest)| rest.strip_prefix("11."))
            .and_then(|rest| rest.chars().next())
            .filter(|c| c.is_ascii_uppercase())
            .map(|c| c.to_string())
            .unwrap_or_default();
        let next = p.ota_version.clone();
        if !seen.insert(p.key()) {
            break;
        }
        out.push(p);
        if next.is_empty() || next == ov {
            break;
        }
        ov = next;
    }
    out
}

/// 官方源 + 社区源合并后的查询结果
pub struct Merged {
    pub packages: Vec<Package>,
    /// 售后（降级 / 维修）包单独放，不和版本清单混排：
    /// 它们的版本号是「ColorOS16.1.0」这种，和升级包不可比，混在一起排序会乱。
    pub aftersale: Vec<Package>,
    /// 社区源是否用上了（未用上时界面不显示社区相关文案）
    pub community_ok: bool,
    /// 售后通道这次的结果。列表为空时界面要靠它区分「本来没有」和「这次没查通」。
    pub after_state: AfterState,
}

/// 售后通道的查询结局
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AfterState {
    /// 查通了（包可能为 0 个，但查询本身有结果）
    Ok,
    /// 官方 OTA 里就没有 `oplus_separate_soft`：这台机器官方没提供售后包
    NoProject,
    /// 项目号或降级服务器这次没问出结果，不代表没有；值得再点一次「重新扫描」
    Unknown,
}

/// 查询某机型的全量包：官方 A–Z 分支扫描 + 社区源版本清单，按版本号去重合并。
///
/// 官方源永远是主路径：它给出的包带 size / 补丁 / MD5，可直接下载。
/// 社区源只做补充（补历史版本），并负责解释「为什么很多旧版本没列出来」。
pub fn query_merged(code: &str, name: &str, brand: &str) -> Merged {
    // 分支扫描、升级链、社区源、售后源互不依赖，四条线同时跑；
    // 总耗时由最慢的那条决定，而不是四者相加。
    let (soft_tx, soft_rx) = std::sync::mpsc::channel::<Soft>();

    // 分支扫描顺带把项目号捞出来，通过 channel 交给售后那条线。
    // 这样官方 OTA 每个机型只被打一遍：售后通道不再自己重复问一轮。
    let branch_code = code.to_string();
    let branches = std::thread::spawn(move || {
        let (list, soft) = query_all_branches_harvest(&branch_code);
        let _ = soft_tx.send(soft);
        list
    });

    let chain_code = code.to_string();
    let chain = std::thread::spawn(move || query_chain(&chain_code));

    // 售后源等分支扫描把项目号递过来。只有「递过来的结论是未知」或「等超时」
    // 这两种情况才自己去问一次官方 OTA —— 那多半意味着官方接口这一轮整体降级了，
    // 正是售后分区凭空消失的那一刻，值得用带多轮重试的独立查询兜一次底。
    let soft_code = code.to_string();
    let aftersale = std::thread::spawn(move || {
        let soft = match soft_rx.recv_timeout(Duration::from_secs(25)) {
            Ok(Soft::Unknown) | Err(_) => separate_soft(&soft_code),
            Ok(s) => s,
        };
        crate::core::downgrade::query_with(&soft_code, soft)
    });

    let (vc, vn, vb) = (code.to_string(), name.to_string(), brand.to_string());
    let community = std::thread::spawn(move || {
        // 社区源是第三方站点，页面结构随时可能变；它 panic 了不能拖垮整个查询
        std::panic::catch_unwind(|| crate::core::violet::packages(&vc, &vn, &vb))
    });

    let mut all = branches.join().unwrap_or_default();
    let mut seen: std::collections::HashSet<String> = all.iter().map(|p| p.key()).collect();
    if let Ok(extra) = chain.join() {
        for p in extra {
            if seen.insert(p.key()) {
                all.push(p);
            }
        }
    }

    let community = community
        .join()
        .ok()
        .and_then(|r| r.ok())
        .unwrap_or_default();
    let community_ok = !community.is_empty();
    for p in community {
        if seen.insert(p.key()) {
            all.push(p);
        }
    }
    // 排序：版本号新的在前。
    //
    // 社区包没有发布时间，只能靠版本号排序；而版本号的数字段本身就是发布顺序
    // （16.0.10.500 > 15.0.0.1300 > 14.1.0.805），两个源共用同一把尺子，
    // 排序后自然交错，不会出现「官方一堆在上、社区一堆在下」。
    // 同一版本号时官方在前：官方包信息完整、可直接下载。
    all.sort_by(|a, b| {
        version_cmp(&b.version, &a.version)
            .then_with(|| (a.source == Source::Community).cmp(&(b.source == Source::Community)))
            .then_with(|| a.version.cmp(&b.version))
    });
    // 售后包不参与上面的排序与去重：它们的版本号是 ColorOS 自己的号，
    // 和升级包不在同一个坐标系里，混排只会让列表看起来乱。
    let after = aftersale
        .join()
        .unwrap_or_else(|_| crate::core::downgrade::AfterSale::unknown());
    let after_state = match &after.soft {
        Soft::Found(_) if !after.list.is_empty() => AfterState::Ok,
        Soft::Found(_) => AfterState::Unknown,
        Soft::Absent => AfterState::NoProject,
        Soft::Unknown => AfterState::Unknown,
    };
    Merged {
        packages: all,
        aftersale: after.list,
        community_ok,
        after_state,
    }
}

/// 把 manualUrl 解析成真正的 CDN 直链
/// 有些机型 manualUrl 本身就是直链，有些是 downloadCheck? 需 302 换签名
pub fn resolve_direct_url(manual_url: &str) -> Result<(String, i64), String> {
    if manual_url.is_empty() {
        return Err("没有可用的下载地址".into());
    }
    if !manual_url.contains("downloadCheck") {
        let exp = parse_expires(manual_url);
        return Ok((manual_url.to_string(), exp));
    }
    let agent = ureq::AgentBuilder::new()
        .redirects(0)
        .timeout_connect(std::time::Duration::from_secs(15))
        .timeout_read(std::time::Duration::from_secs(20))
        .build();
    let resp = agent
        .get(manual_url)
        .set("userId", "oplus-ota|00000001")
        .set("Range", "bytes=0-")
        .call();
    match resp {
        Ok(r) => {
            let loc = r.header("location").unwrap_or("");
            if !loc.is_empty() {
                let exp = parse_expires(loc);
                Ok((loc.to_string(), exp))
            } else {
                Err("没有拿到跳转地址".into())
            }
        }
        Err(ureq::Error::Status(302, r)) => {
            let loc = r.header("location").unwrap_or("").to_string();
            let exp = parse_expires(&loc);
            Ok((loc, exp))
        }
        Err(ureq::Error::Status(code, _)) => Err(format!("解析直链失败，服务器返回 {}", code)),
        Err(e) => Err(format!("网络错误: {}", e)),
    }
}

/// 从签名直链里解析过期时间（unix 秒）；解析不到返回 0
pub fn parse_expires(url: &str) -> i64 {
    for key in ["Expires=", "x-oss-expires="] {
        if let Some(pos) = url.find(key) {
            let rest = &url[pos + key.len()..];
            let num: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(v) = num.parse::<i64>() {
                return v;
            }
        }
    }
    0
}
