//! 售后（降级 / 维修）全量包协议：对接 ColorOS 的 downgrade.coloros.com「query-v3」。
//! 官方 OTA 只给升级包，售后刷机（降级 / 维修）用的是这台服务器另签的全量包，其直链
//! 长期有效、支持 Range，所以它是官方源之外唯一能补「旧系统」的来源。
//! 与 ota.rs 的 AES-CTR（流加密、无完整性校验）不同，这里用 AES-256-GCM（AEAD，
//! 密文自带 16 字节 tag），且加密只出现在 protectedKey 与 body 的 deviceId 两处。
//! 服务端约一半请求会 500 或报 AES decrypt error，故整包重试且每次换新会话密钥。

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use rand::{Rng, RngCore};
use rsa::{pkcs8::DecodePublicKey, Oaep, RsaPublicKey};
use serde_json::{json, Value};
use sha1::Sha1;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::core::meta;
use crate::core::ota::{self, Package, Source};

const ENDPOINT: &str = "https://downgrade.coloros.com/downgrade/query-v3";

/// 协议写死的 RSA 公钥（SPKI / DER 294 字节 / 2048 位），只用来包一层会话密钥。
/// 别自己加 PEM 头尾：这里走 from_public_key_der，加了壳反而解析失败。
const PUBKEY_B64: &str = "MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAmeQzr0TIbtwZFnDXgatg6xP9SlNBFho1NTdFQ27SKDF+dBEEfnG9BqRw0na0DUqtpWe2CUtldbU33nnJ0KB6z7y5f+89o9n8mJxIbh952gpskBxyrhCfpYHV5mt/n9Tkm8OcQWLRFou7/XITuZeZejfUTesQjpfOeCaeKyVSoKQc6WuH7NSYq6B37RMyEn/1+vo8XuHEKD84p29KGpyGI7ZeL85iOcwBmOD6+e4yideH2RatA1SzEv/9V8BflaFLAWDuPWUjA2WgfOvy5spYmp/MoMOX4P0d+AkJ9Ms6PUXEUBsbOACmaMFyLCLHmd18+UeGdJR/3I15sXKbJhKerwIDAQAB";

const NV_CARRIER: &str = "10010111";
const NEGOTIATION_VERSION: i64 = 1_636_449_646_204;
const KEY_LEN: usize = 32;
const IV_LEN: usize = 12;
/// 请求里必须带一个「已在服务器建档」的 64 位十六进制 DUID，否则服务端直接回
/// 1004「duid查询guid为空」，一条记录都拿不到（用 64 个 0 就是这个结果，这也是本模块
/// 最初查不到任何售后包的真正原因）。这里用公开参考实现里带的那种查询用 DUID：
/// 它只参与建档校验，不绑定机身，换任何机型查出来的都是该机型自己的包。
const DUIDS: [&str; 1] = [
    "498A44DF1BEC4EB19FBCB3A870FCACB4EC7D424979CC9C517FE7B805A1937746",
];
/// 服务端约一半概率抽风，实测 8 次内必能拿到结果；再多只会让界面等得难受
const MAX_TRIES: usize = 8;
const RETRY_WAIT_MS: u64 = 600;

/// 售后包查询结果：包本身，外加「为什么是空的」。
///
/// 只回一个空 Vec 的话，界面分不清「这台机器官方就没有售后包」和「这次没查通」，
/// 用户看到的就是售后分区无声无息地不见了 —— 那正是「怎么只有某个机型有」的困惑来源。
pub struct AfterSale {
    pub list: Vec<Package>,
    pub soft: ota::Soft,
}

impl AfterSale {
    fn none(soft: ota::Soft) -> Self {
        Self { list: Vec::new(), soft }
    }

    /// 售后那条线程自己 panic 了（网络库或解析里不该发生的意外）时的兜底。
    /// 结论只能算「未知」：不能因为线程崩了就告诉用户「这台机器没有售后包」。
    pub fn unknown() -> Self {
        Self::none(ota::Soft::Unknown)
    }
}

/// 查询某机型的售后（降级 / 维修）全量包。失败一律返回空清单，绝不 panic。
///
/// 项目号（P）由调用方给。降级服务器只认官方 OTA 里的 `oplus_separate_soft`，
/// 它是两条协议之间唯一的桥；拿不到就等于没有售后包，不必再打扰降级服务器。
///
/// 之所以由调用方传入而不是自己查：合并查询时分支扫描本来就要把这 26 次 OTA
/// 请求打一遍，项目号顺手就有；售后通道再自己问一轮，等于把同一批请求打两遍，
/// 而那正是最容易降级的接口。
pub fn query_with(code: &str, soft: ota::Soft) -> AfterSale {
    let code = code.trim();
    if code.is_empty() {
        return AfterSale::none(ota::Soft::Unknown);
    }
    let ota::Soft::Found(prj) = soft.clone() else {
        return AfterSale::none(soft);
    };
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(30))
        .build();

    for attempt in 0..MAX_TRIES {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(RETRY_WAIT_MS));
        }
        // 不区分「服务端抖动」与「真的没包」：两者在 HTTP 层长得一样（500 / 解密失败 /
        // 空 data），重发几次的代价远小于误报「未查到售后包」。
        if let Ok(list) = try_once(&agent, code, &prj) {
            return AfterSale { list: enrich(list), soft };
        }
    }
    // 项目号是拿到了，降级服务器却 8 次都没给结果：售后分区仍然该显示，
    // 只是标明「本次没查到」而不是「没有」，用户重试一次往往就有了。
    AfterSale::none(soft)
}

/// 单次请求：换一套新会话密钥，拼 body，发出去，解析响应。每次重试都重生成 session_key /
/// iv / protectedKey：服务端的 500 有一部分正是上一把密钥没协商上。
fn try_once(agent: &ureq::Agent, code: &str, prj: &str) -> Result<Vec<Package>, String> {
    let mut session_key = [0u8; KEY_LEN];
    let mut iv = [0u8; IV_LEN];
    let mut rng = rand::thread_rng();
    rng.fill_bytes(&mut session_key);
    rng.fill_bytes(&mut iv);

    // 每次重试换一个 DUID：单个 DUID 偶发也会被判无效，换一个往往就过了
    let device_plain = DUIDS[rand::thread_rng().gen_range(0..DUIDS.len())].to_string();
    // body 里的 deviceId 要加密，请求头里的那个是明文，两处必须是同一个值
    let device_cipher = gcm_encrypt(&session_key, &iv, device_plain.as_bytes())?;

    // protectedKey 是一次 RSA 运算，放在 json! 外求值，出错就直接短路
    let protected = wrap_key(&session_key)?;
    let cipher_info = json!({ "downgrade-server": {
        "negotiationVersion": NEGOTIATION_VERSION,
        "protectedKey": protected,
        "version": now_secs().to_string()
    }})
    .to_string();

    let body = json!({
        "model": code,
        "nvCarrier": NV_CARRIER,
        "prjNum": prj,
        // 参考实现会带一个短序列号；服务端并不校验它，但缺字段更容易被判成异常请求
        "serialNo": "a1b2c3d4",
        // 服务器只按 model + prjNum 查库，分支字母不参与筛选，给个合法形式即可
        "otaVersion": format!("{}_11.A", code),
        "deviceId": {
            "cipher": B64.encode(&device_cipher),
            "iv": B64.encode(iv)
        }
    })
    .to_string();

    let resp = agent
        .post(ENDPOINT)
        .set("Content-Type", "application/json; charset=UTF-8")
        .set("cipherInfo", &cipher_info)
        // 请求头里的 deviceId 是明文，与 body 里那个密文是两回事，别混用
        .set("deviceId", &device_plain)
        .set("Connection", "close")
        .send_string(&body);

    // 5xx 也要读 body：服务端的报错正文里就是 AES decrypt 之类的线索
    let text = match resp {
        Ok(r) => r.into_string().map_err(|e| format!("读取响应失败: {}", e))?,
        Err(ureq::Error::Status(_, r)) => r.into_string().map_err(|e| e.to_string())?,
        Err(e) => return Err(format!("网络错误: {}", e)),
    };

    let root = parse_or_decrypt(&text, &session_key)?;
    packages_of(&root)
}

/// 解析响应：明文直接用，加密形态（根对象带 cipher / iv）则当场解密。先解成 owned 的
/// Vec 再交出 root：被借用期间没法整体 move。
fn parse_or_decrypt(text: &str, key: &[u8; KEY_LEN]) -> Result<Value, String> {
    let root: Value = serde_json::from_str(text).map_err(|e| format!("响应不是 JSON: {}", e))?;
    let decrypted = match (
        root.get("cipher").and_then(Value::as_str),
        root.get("iv").and_then(Value::as_str),
    ) {
        (Some(cipher), Some(iv)) => {
            let data = B64.decode(cipher).map_err(|e| format!("响应 cipher 解码失败: {}", e))?;
            let nonce = B64.decode(iv).map_err(|e| format!("响应 iv 解码失败: {}", e))?;
            Some(gcm_decrypt(key, &nonce, &data)?)
        }
        // 正常情况就是明文，这才是主路径
        _ => None,
    };
    match decrypted {
        Some(plain) => {
            serde_json::from_slice(&plain).map_err(|e| format!("响应解密后不是 JSON: {}", e))
        }
        None => Ok(root),
    }
}

/// 取 data.downgradeVoList 的每条记录；任何一层缺失都算本次失败（交由上层重试）。
fn packages_of(root: &Value) -> Result<Vec<Package>, String> {
    // code 有时是数字有时是字符串，两种都收；缺失就按失败处理（交给上层重试）
    let resp_code = root
        .get("code")
        .and_then(|v| v.as_i64().or_else(|| v.as_str()?.trim().parse::<i64>().ok()))
        .unwrap_or(-1);
    // 非 200 也当作本次失败：服务端抖动同样是这个形态，交给上层重试
    if resp_code != 200 {
        let msg = root.get("msg").or_else(|| root.get("message"));
        let msg = msg.and_then(Value::as_str).unwrap_or("未说明原因");
        return Err(format!("查询失败 ({}): {}", resp_code, msg));
    }
    let list = root.get("data").and_then(|d| d.get("downgradeVoList"));
    let list = list.and_then(Value::as_array).ok_or("没有 downgradeVoList")?;
    Ok(list.iter().map(to_package).collect())
}

/// 用 ZIP 里的 metadata 补齐售后包的真实身份。
///
/// 服务器只给 colorosVersion（"ColorOS16.1.0"），同一机型的所有售后包在这
/// 一项上完全一样，界面照着显示等于没说。而 A/B 全量包自带
/// META-INF/com/android/metadata，里面才有真正的 version_name / 补丁 / 构建时间。
/// 每个包要额外发一次 Range 请求，所以并行跑；任何一个失败都只是「这个包不补」，
/// 绝不影响整次查询。
fn enrich(list: Vec<Package>) -> Vec<Package> {
    if list.is_empty() {
        return list;
    }
    let metas: Vec<Option<meta::Meta>> = std::thread::scope(|s| {
        let handles: Vec<_> = list
            .iter()
            .map(|p| {
                let url = p.manual_url.clone();
                s.spawn(move || {
                    if url.is_empty() {
                        None
                    } else {
                        meta::read(&url)
                    }
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap_or(None))
            .collect()
    });
    list.into_iter()
        .zip(metas)
        .map(|(mut pkg, m)| {
            fill(&mut pkg, m);
            pkg
        })
        .collect()
}

/// 把读到的 metadata 覆盖到包上。只覆盖真有值的字段，
/// 读不到（或字段缺）时保留服务器给的原值，界面不会因此变空。
fn fill(pkg: &mut Package, m: Option<meta::Meta>) {
    // 服务器给的 publishedTime 其实是「此刻」，每次请求都在变；
    // metadata 里的 post-timestamp 才是真正的构建时间，有它就该用它。
    let Some(m) = m else { return };
    if !m.version.is_empty() {
        pkg.version = m.version;
    }
    if !m.ota_version.is_empty() {
        pkg.ota_version = m.ota_version;
    }
    if !m.android.is_empty() {
        pkg.android = m.android;
    }
    if !m.security_patch.is_empty() {
        pkg.security_patch = m.security_patch;
    }
    if !m.published.is_empty() {
        pkg.published = m.published;
    }
    if pkg.branch.is_empty() {
        pkg.branch = branch_of(&pkg.ota_version);
    }
}

/// 从内部版本号里取出分支字母："RMX3888_11.F.35_2350_202608121732" → "F"。
/// 售后包本来没有分支概念，但它的 ota_version 就是按分支编的，取出来能让
/// 卡片左侧那条竖线按分支上色，也方便用户对照官方列表。
fn branch_of(ota_version: &str) -> String {
    let mid = ota_version.split('_').nth(1).unwrap_or("");
    let letter = mid.split('.').nth(1).unwrap_or("");
    if letter.len() == 1 && letter.chars().all(|c| c.is_ascii_alphabetic()) {
        letter.to_ascii_uppercase()
    } else {
        String::new()
    }
}

/// 把一条 downgradeVoList 记录映射成界面用的 Package。
/// downloadUrl 是裸直链，链接长期有效且支持 Range（实测 206），故 link_ok 直接给
/// Some(true)，上层不必再探测；没有 downloadUrl 的记录照常映射，交给上层显示。
fn to_package(v: &Value) -> Package {
    let android = text_of(v.get("androidVersion"));
    let coloros = text_of(v.get("colorosVersion"));
    // 版本号优先用 ColorOS 版本，退化到 Android 版本，再退化到固定文案（界面不能空着）
    let version = [coloros, android.clone()]
        .into_iter()
        .find(|s| !s.is_empty())
        .unwrap_or_else(|| "售后全量包".to_string());

    Package {
        version,
        android,
        size: count_of(v.get("fileSize")),
        manual_url: text_of(v.get("downloadUrl")),
        published: stamp_of(v.get("publishedTime")),
        security_patch: String::new(),
        // 售后包没有 A–Z 分支，界面按 source 上色，这里留空
        branch: String::new(),
        md5: text_of(v.get("fileMd5")),
        ota_version: text_of(v.get("otaVersion")),
        source: Source::AfterSale,
        vio: None,
        link_ok: Some(true),
        note: text_of(v.get("versionIntroduction")),
    }
}

/// AES-256-GCM 加密。返回「密文 || 16 字节 tag」，正是降级服务要的拼法 —— aead 的
/// encrypt 会把 tag 追加在缓冲末尾，所以这里不能再去手动拼一遍。
fn gcm_encrypt(key: &[u8; KEY_LEN], iv: &[u8; IV_LEN], plain: &[u8]) -> Result<Vec<u8>, String> {
    // 固定 32 字节密钥，用 Key 的 From 转换即可，不必走会返回 Result 的 new_from_slice
    let cipher = Aes256Gcm::new(key.into());
    cipher.encrypt(Nonce::from_slice(iv), plain).map_err(|_| "AES-GCM 加密失败".to_string())
}

/// AES-256-GCM 解密，入参必须是「密文 || 16 字节 tag」。GCM 的 nonce 固定 12 字节，
/// 长度不对就是形态不符预期：报错让上层重试，绝不猜个长度硬解。
fn gcm_decrypt(key: &[u8; KEY_LEN], iv: &[u8], data: &[u8]) -> Result<Vec<u8>, String> {
    let nonce: [u8; IV_LEN] = iv
        .try_into()
        .map_err(|_| format!("iv 长度应为 {} 字节，实际 {}", IV_LEN, iv.len()))?;
    let cipher = Aes256Gcm::new(key.into());
    cipher
        .decrypt(Nonce::from_slice(&nonce), data)
        .map_err(|_| "响应解密失败".to_string())
}

/// 用服务端公钥 RSA-OAEP(MGF1-SHA1, SHA1) 包住会话密钥。关键是：被加密的明文是
/// base64(session_key) 这串文本的字节，不是裸 32 字节密钥 —— 服务端先 base64 解码才
/// 当 AES 密钥用，传裸字节会一路报 500 / AES decrypt error。
fn wrap_key(key: &[u8]) -> Result<String, String> {
    let der = B64.decode(PUBKEY_B64).map_err(|e| format!("公钥解码失败: {}", e))?;
    let pubkey = RsaPublicKey::from_public_key_der(&der).map_err(|e| e.to_string())?;
    let mut rng = rand::thread_rng();
    let plain = B64.encode(key);
    let enc = pubkey
        .encrypt(&mut rng, Oaep::new::<Sha1>(), plain.as_bytes())
        .map_err(|e| format!("保护会话密钥失败: {}", e))?;
    Ok(B64.encode(enc))
}

/// 取文本字段；服务器偶尔用数字表示版本或补丁号，一并转成文本。
fn text_of(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.trim().to_string(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

/// fileSize 有时是数字有时是字符串，统一按 u64 解析；解析不出给 0，界面显示大小未知。
fn count_of(v: Option<&Value>) -> u64 {
    match v {
        Some(Value::Number(n)) => n.as_u64().or_else(|| n.as_f64().map(|f| f.max(0.0) as u64)),
        Some(Value::String(s)) => s.trim().parse().ok(),
        _ => None,
    }
    .unwrap_or(0)
}

/// 毫秒时间戳转十进制字符串（界面自行格式化）；本来就是字符串的原样带走。
fn stamp_of(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.trim().to_string(),
        other => match count_of(other) {
            0 => String::new(),
            t => t.to_string(),
        },
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}