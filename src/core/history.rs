//! 下载历史记录仓库：记住每个包下到哪、下没下完，重启后还能接着续传。
//!
//! 为什么需要：全量包动辄几个 G，用户经常关掉程序第二天再下。进度只存在内存里
//! 的话，下次启动就什么都不剩，用户只能从头再来一遍。
//! 存在哪：Windows 下放 %APPDATA%\OujiazhenOtaTool\history.json（可用环境变量
//! OTA_TOOL_DIR 改到别处），绝不写进下载目录 —— 那里只该有用户下载的包。
//! 三种状态：Paused 还有 .partstate 缓存可以续传；Done 已下完且缓存已清掉；
//! Failed 上次是出错停下的。磁盘才是实况，所以启动时要调 refresh() 校正一次。
use crate::core::download;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// 历史文件名
const FILE_NAME: &str = "history.json";

/// 写盘用的临时文件名。
///
/// 先写它再 rename 覆盖正式文件：中途断电最多留下这个半截垃圾，
/// history.json 本身永远是上一个完整版本，不会变成半截 JSON 读不出来。
const TMP_NAME: &str = "history.json.tmp";

/// %APPDATA% 下的应用私有目录名
const APP_DIR: &str = "OujiazhenOtaTool";

/// 非 Windows（以及 APPDATA 缺失）时用的家目录子目录，带点表示隐藏
const DOT_DIR: &str = ".oujiazhen-ota";

/// 一条历史记录
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Entry {
    /// 显示用版本号，如 "RMX3888_16.0.10.500(CN01)"
    pub version: String,
    /// 机型代号，如 "RMX3888"
    pub model: String,
    /// 机型中文名，如 "真我GT5 Pro"
    pub model_name: String,
    /// 完整的包信息：历史页要用它重新换取直链续传，所以整份存档而不是只存路径
    pub pkg: crate::core::ota::Package,
    /// 目标文件的**绝对路径**，同时是记录的唯一键
    pub target: String,
    /// 下载状态
    pub status: Status,
    /// 已落盘字节（来自 .partstate 位图或文件大小）
    pub bytes: u64,
    /// 总字节
    pub total: u64,
    /// 最后更新时间，unix 秒
    pub updated: i64,
    /// 校验结果：None 未校验 / Some(true) 通过 / Some(false) 不符
    pub md5_ok: Option<bool>,
}

/// 一条记录的下载状态
///
/// 三种状态互斥且可以直接由磁盘推断，所以 refresh() 每次启动都能重算，
/// 不需要（也不该）把状态当成用户数据长期信任。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Serialize, serde::Deserialize)]
pub enum Status {
    /// 有 .partstate 缓存，可以续传
    #[default]
    Paused,
    /// 正在下载。
    ///
    /// 只在本次运行内出现：磁盘上看不出「正在下」和「停在半路」的区别，
    /// 所以 refresh()（启动时按磁盘实况校正）永远不会产出这个状态，
    /// 重启后一定会落回 Paused —— 这正是我们想要的结果。
    Downloading,
    /// 已下完（.partstate 已被删除，文件存在）
    Done,
    /// 出错停止
    Failed,
}

impl Status {
    /// 历史页上的角标文案（历史页接好之前暂时没人调用，先不报未使用警告）
    #[allow(dead_code)]
    pub fn label(self) -> &'static str {
        match self {
            Status::Paused => "已暂停",
            Status::Downloading => "下载中",
            Status::Done => "已完成",
            Status::Failed => "失败",
        }
    }
}

/// 历史记录仓库
pub struct Store {
    /// 存档路径（构造时定下来，之后不再改，保证 save 与 load 一定指向同一文件）
    path: PathBuf,
    /// 内存里的记录，始终按 updated 倒序
    list: Vec<Entry>,
}

impl Store {
    /// 从磁盘加载；文件不存在/损坏时返回空仓库（绝不 panic）
    pub fn load() -> Store {
        let path = store_path();
        let mut store = Store {
            path,
            list: Vec::new(),
        };
        // 读不到（第一次运行）和读到半截 JSON 都走同一条路：空着开始。
        // 历史记录丢了顶多重新下一次，崩溃/弹窗才是真的不能接受。
        if let Ok(txt) = fs::read_to_string(&store.path) {
            if let Ok(list) = serde_json::from_str::<Vec<Entry>>(&txt) {
                store.list = list;
            }
        }
        store.sort_by_updated();
        store
    }

    /// 写回磁盘（自动建目录；失败静默忽略，不 panic）
    pub fn save(&self) {
        // 序列化失败（理论上不会）就别动磁盘：保持旧文件总好过写个空数组
        let text = match serde_json::to_string_pretty(&self.list) {
            Ok(t) => t,
            Err(_) => return,
        };
        if let Some(parent) = self.path.parent() {
            // 裸文件名（相对路径兜底）的 parent 是空串，这时不需要建目录
            if !parent.as_os_str().is_empty() && fs::create_dir_all(parent).is_err() {
                return;
            }
        }
        let tmp = self.path.with_file_name(TMP_NAME);
        if fs::write(&tmp, text.as_bytes()).is_err() {
            return;
        }
        // rename 是同一目录内的原子替换：要么看到旧文件，要么看到新文件
        let _ = fs::rename(&tmp, &self.path);
    }

    /// 全部记录，按 updated 倒序
    pub fn entries(&self) -> &[Entry] {
        &self.list
    }

    /// 插入或按 target 覆盖已有记录，并立即 save()
    pub fn upsert(&mut self, e: Entry) {
        // 时间戳由仓库统一盖：调用方只管把包、路径、状态填进来，
        // 免得某个调用点漏写 updated，排序就全乱套了。
        let entry = Entry {
            updated: now_secs(),
            ..e
        };
        match self.list.iter_mut().find(|old| old.target == entry.target) {
            Some(slot) => *slot = entry,
            None => self.list.push(entry),
        }
        self.save();
    }

    /// 按 target 删除一条，并立即 save()
    pub fn remove(&mut self, target: &str) {
        self.list.retain(|e| e.target != target);
        self.save();
    }

    /// 清空全部并 save()
    pub fn clear(&mut self) {
        self.list.clear();
        self.save();
    }

    /// 重新按磁盘实况校正所有记录：状态、进度、还有「文件已经不在」的清理
    ///
    /// 判定顺序就是三分支：
    /// 1. .partstate 在 → 没下完，可以续传，进度按位图算；
    /// 2. .partstate 不在但文件在 → 下完了（下载器清缓存只在收尾时做）；
    /// 3. 两个都不在 → 文件被用户删了，记录留着只会误导，直接丢弃。
    pub fn refresh(&mut self) {
        let mut kept: Vec<Entry> = Vec::with_capacity(self.list.len());
        for mut e in std::mem::take(&mut self.list) {
            let file = Path::new(&e.target);
            // state_path 是下载器的公开接口，续传缓存的位置以它为准
            let part = download::state_path(file);

            // 分支 1：还有分块缓存，说明是暂停/中断，进度从位图里数出来
            if part.is_file() {
                e.status = Status::Paused;
                match read_part_state(&part) {
                    Some((total, done)) => {
                        // 总字节以缓存里记的为准（那是本次下载真正在下的长度，
                        // 可能和清单里的 size 有几百字节的出入）
                        if total > 0 {
                            e.total = total;
                        }
                        e.bytes = done;
                    }
                    None => {
                        // 缓存文件坏了/被改过：退回文件实际长度，
                        // 再拿不到就保留原值，至少不要把进度清零
                        if let Ok(meta) = fs::metadata(file) {
                            e.bytes = meta.len();
                        }
                    }
                }
                kept.push(e);
                continue;
            }

            // 分支 2 / 3：没有缓存，只能看最终文件在不在
            match fs::metadata(file) {
                Ok(meta) if meta.is_file() => {
                    // 下载器只在收尾（校验通过）时才删 .partstate，所以「没缓存 + 有文件」
                    // 就是真的下完了；此时文件长度才是最可靠的实况，进度直接按它填满。
                    let len = meta.len();
                    e.status = Status::Done;
                    e.bytes = len;
                    e.total = len;
                    kept.push(e);
                }
                // 分支 3：文件和缓存都没了，这条记录作废
                _ => {}
            }
        }
        self.list = kept;
        self.sort_by_updated();
        // 校正结果要落盘，否则下次启动又得从头算一遍（大文件遍历是有代价的）
        self.save();
    }

    /// 按 updated 倒序排序（稳定排序，同一秒内的记录保持原有先后，顺序不会乱跳）
    fn sort_by_updated(&mut self) {
        self.list.sort_by(|a, b| b.updated.cmp(&a.updated));
    }
}

/// 历史文件位置
///
/// 注意这里不复用「下载目录」那套逻辑：history.json 属于程序自己的数据，
/// 放在用户的下载目录里会把它弄脏，用户明确要求过下载目录只放包。
pub fn store_path() -> PathBuf {
    // 显式指定优先：便携版/测试用 OTA_TOOL_DIR 把数据挪到别处
    if let Ok(dir) = std::env::var("OTA_TOOL_DIR") {
        if !dir.trim().is_empty() {
            return PathBuf::from(dir).join(FILE_NAME);
        }
    }
    #[cfg(windows)]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            if !appdata.trim().is_empty() {
                return PathBuf::from(appdata).join(APP_DIR).join(FILE_NAME);
            }
        }
        if let Ok(profile) = std::env::var("USERPROFILE") {
            if !profile.trim().is_empty() {
                return PathBuf::from(profile).join(DOT_DIR).join(FILE_NAME);
            }
        }
        PathBuf::from(FILE_NAME)
    }
    #[cfg(not(windows))]
    {
        if let Ok(home) = std::env::var("HOME") {
            if !home.trim().is_empty() {
                return PathBuf::from(home).join(DOT_DIR).join(FILE_NAME);
            }
        }
        PathBuf::from(FILE_NAME)
    }
}

/// 解析 .partstate，返回 (总字节, 已下字节)
///
/// 文件格式是两行纯文本，与 download.rs 里 save_state 写出的完全一致：
/// 第 1 行总字节数（十进制），第 2 行是每块的 '0'/'1' 位图，第 i 位对应
/// 第 i 块。下载器那边 load_state 是私有的，为了读一个进度去改它的可见性
/// 不值当，这里按格式自己解析一遍；格式若变，两边都要跟着改。
///
/// 已下字节不能简单等于「'1' 的个数 × CHUNK」：最后一块通常不满 8MB，
/// 所以每块按 (total - i*CHUNK) 与 CHUNK 取小累加，否则进度会虚高。
fn read_part_state(part: &Path) -> Option<(u64, u64)> {
    let txt = fs::read_to_string(part).ok()?;
    let mut lines = txt.lines();
    let total: u64 = lines.next()?.trim().parse().ok()?;
    // 位图行缺失/为空时按「一块都没下」处理：缓存被截断时这是最诚实的解读
    let bits = lines.next().map(str::trim).unwrap_or("");
    let mut done: u64 = 0;
    for (i, c) in bits.chars().enumerate() {
        if c != '1' {
            continue;
        }
        // 用 saturating_sub：位图比实际长度长（缓存串了/被改过）时不会下溢 panic
        let off = (i as u64).saturating_mul(download::CHUNK);
        done = done.saturating_add(total.saturating_sub(off).min(download::CHUNK));
    }
    Some((total, done))
}

/// 当前 unix 秒。
///
/// duration_since 只在系统时间被调到 1970 之前才会 Err，那时给 0 即可 ——
/// 为了一个时钟异常就让整个历史功能崩掉不值得。
fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
