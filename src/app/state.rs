//! 应用状态与业务动作（不含任何绘制代码）
use crate::core::download::{self, Progress};
use crate::core::history;
use crate::core::ota::{self, Package, Source};
use crate::core::search::{self, Hit};
use crate::core::violet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};

/// 售后分区收起时显示几张卡。
///
/// 售后通道一个机型常给三十多个包（实测 RMX3706 有 34 个），全铺开会把
/// 「可用全量包」整段推到屏幕外 —— 那个分区才是日常要用的。
/// 1180x820 下一张卡约 130px：预留 2 张，才能让下面「可用全量包」也露出实卡，
/// 而不是只剩一行标题。想看全的点「展开全部」。
pub const AFTER_PREVIEW: usize = 2;

/// 状态栏语气
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum Tone {
    #[default]
    Info,
    Ok,
    Warn,
    Err,
}

/// 正在进行中的一个下载任务
pub struct ActiveDownload {
    pub progress: Arc<Mutex<Progress>>,
    pub cancel: Arc<AtomicBool>,
    pub name: String,
    pub target: PathBuf,
    /// 原始包信息：暂停后点「继续下载」要用它重新挂起同一个任务，
    /// 这样 [`download::start`] 能从 .partstate 里读回已完成的分块继续下。
    pub pkg: Package,
}

/// 已解析出来的官方直链
#[derive(Clone)]
pub struct LinkInfo {
    /// 完整的官方签名直链（约 10 分钟过期）；界面只显示剩余时间，故此处仅作留档。
    #[allow(dead_code)]
    pub url: String,
    /// unix 秒，0 表示未知
    pub expires: i64,
    /// 这份直链属于哪个包。用 [`Package::id`] 而不是版本号：
    /// 售后包补齐 metadata 之后，版本号会和官方升级包一模一样，
    /// 拿版本号比对会让两张卡同时亮成「已复制」。
    pub id: String,
    /// 直链来自哪个源。三种源的有效期说法不一样：官方 OTA 是 10 分钟签名链，
    /// 社区源要标作者，售后直链是长期有效的裸链 —— 文案不能一概而论。
    pub source: Source,
}

/// 一次合并查询的返回值：(机型代号, 版本清单, 售后包, 社区源是否可用, 售后通道结局)
type QueryResult = Result<(String, Vec<Package>, Vec<Package>, bool, ota::AfterState), String>;

/// 当前选中的机型
#[derive(Clone)]
pub struct Model {
    pub code: String,
    pub name: String,
    pub brand: String,
}

pub struct App {
    pub keyword: String,
    pub results: Vec<Hit>,
    pub selected: Option<usize>,
    pub current: Option<Model>,
    pub packages: Vec<Package>,
    /// 售后（降级 / 维修）包。单独一栏，不和版本清单混排 —— 版本号不可比。
    pub aftersale: Vec<Package>,
    /// 售后通道本次的结局。列表为空时界面靠它区分「本来没有」和「这次没查通」。
    pub after_state: ota::AfterState,
    /// 售后分区是否已展开。
    ///
    /// 售后通道一个机型常给三十多个包（RMX3706 有 34 个），全铺开会把
    /// 「可用全量包」整段推到屏幕外；默认收起到前几个，点「展开全部」再看。
    pub after_open: bool,
    query_rx: Option<Receiver<QueryResult>>,
    /// 社区源本次是否用上了（用不上时界面不展示相关文案）
    pub community_ok: bool,
    /// 社区源的链接可用性探测结果（版本键 → 文件大小）
    probe_rx: Option<Receiver<Vec<(String, Option<u64>)>>>,
    /// 还没拿到文件大小的社区包，等后台问出来再启动下载
    pending: Option<Package>,
    size_rx: Option<Receiver<(String, Result<u64, String>)>>,
    pub querying: bool,
    pub status: String,
    pub tone: Tone,
    pub save_dir: String,
    pub threads: usize,
    pub download: Option<ActiveDownload>,
    pub link: Option<LinkInfo>,
    link_rx: Option<Receiver<Result<(String, i64), String>>>,
    pub resolving: bool,
    /// 直链对应的包：(身份, 版本号)。身份用于判断哪张卡该亮「已复制」，
    /// 版本号用于判断这条链接是官方签的还是社区源签的。
    link_for: Option<(String, String)>,
    /// 待写入剪贴板的文本（由界面层取出后清空）
    pub pending_copy: Option<String>,
    /// 「数据来源与致谢」弹窗是否打开
    pub about_open: bool,
    /// 历史下载记录（跨会话持久化）
    pub history: history::Store,
    /// 「历史下载」弹窗是否打开
    pub history_open: bool,
    /// 上一次写进历史记录的快照：(目标路径, 状态码, 进度台阶)。
    /// 用来判断「这次的变化值不值得落盘」，避免每帧都写 JSON。
    hist_stamp: Option<(String, u8, u64)>,
}

impl App {
    pub fn new(initial: Option<String>, dir: Option<String>) -> Self {
        let mut app = Self {
            keyword: String::new(),
            results: Vec::new(),
            selected: None,
            current: None,
            packages: Vec::new(),
            aftersale: Vec::new(),
            after_state: ota::AfterState::Unknown,
            after_open: false,
            query_rx: None,
            community_ok: false,
            probe_rx: None,
            pending: None,
            size_rx: None,
            querying: false,
            status: "输入机型名称或代号，例如 GT5 Pro、Find X8、RMX3888".to_string(),
            tone: Tone::Info,
            save_dir: dir.unwrap_or_else(default_dir),
            threads: 16,
            download: None,
            link: None,
            link_rx: None,
            resolving: false,
            link_for: None,
            pending_copy: None,
            about_open: false,
            history: history::Store::load(),
            history_open: false,
            hist_stamp: None,
        };
        // 磁盘才是实况：启动时按 .partstate 与文件是否还在，把上次的记录校正一遍
        app.history.refresh();
        if let Some(kw) = initial {
            app.keyword = kw;
            app.search();
        }
        app
    }

    pub fn set(&mut self, tone: Tone, text: impl Into<String>) {
        self.tone = tone;
        self.status = text.into();
    }

    /// 按关键词搜索机型，并自动查询第一条结果
    pub fn search(&mut self) {
        self.results.clear();
        self.selected = None;
        self.packages.clear();
        self.aftersale.clear();
        self.current = None;
        self.link = None;
        self.link_rx = None;
        let hits = search::search(&self.keyword, 300);
        if hits.is_empty() {
            let code = search::normalize_code(&self.keyword);
            if search::is_valid_code(&code) {
                self.query(code.clone(), code);
            } else {
                self.set(Tone::Warn, "没有匹配的机型，换个关键词试试，或直接输入机型代号");
            }
            return;
        }
        let n = hits.len();
        self.results = hits;
        self.set(Tone::Info, format!("找到 {} 个机型", n));
        self.selected = Some(0);
        self.query_selected();
    }

    /// 把输入框内容直接当作机型代号查询（支持内置表之外的代号）
    pub fn query_code(&mut self) {
        let code = search::normalize_code(&self.keyword);
        if !search::is_valid_code(&code) {
            self.set(Tone::Warn, "代号格式不对，应类似 RMX3888 / PJZ110 / PHK110");
            return;
        }
        self.query(code.clone(), code);
    }

    /// 查询当前选中的机型
    pub fn query_selected(&mut self) {
        let Some(idx) = self.selected else { return };
        let Some(hit) = self.results.get(idx).copied() else { return };
        let m = Model {
            code: hit.code.to_string(),
            name: hit.name.to_string(),
            brand: hit.brand.to_string(),
        };
        self.packages.clear();
        self.aftersale.clear();
        self.link = None;
        self.link_rx = None;
        self.current = Some(m);
        self.query(hit.code.to_string(), hit.name.to_string());
    }

    /// 售后分区可折叠：默认只露前 AFTER_PREVIEW 个。
    pub fn after_visible(&self) -> &[Package] {
        if self.after_open || self.aftersale.len() <= AFTER_PREVIEW {
            &self.aftersale
        } else {
            &self.aftersale[..AFTER_PREVIEW]
        }
    }

    pub fn toggle_after(&mut self) {
        self.after_open = !self.after_open;
    }

    fn query(&mut self, code: String, label: String) {
        self.querying = true;
        self.pending = None;
        self.size_rx = None;
        self.packages.clear();
        self.aftersale.clear();
        self.after_open = false;
        self.community_ok = false;
        self.probe_rx = None;
        self.link = None;
        self.link_rx = None;
        self.set(Tone::Info, format!("正在扫描 {} 的全部分支…", code));
        let (tx, rx) = channel();
        self.query_rx = Some(rx);
        let brand = self
            .current
            .as_ref()
            .map(|m| m.brand.clone())
            .unwrap_or_default();
        let name = label.clone();
        std::thread::spawn(move || {
            let m = ota::query_merged(&code, &name, &brand);
            let res = if m.packages.is_empty() && m.aftersale.is_empty() {
                Err(format!(
                    "{}（{}）查不到全量包。海外机型代号或已下线机型通常没有数据。",
                    name, code
                ))
            } else {
                Ok((
                    code,
                    m.packages,
                    m.aftersale,
                    m.community_ok,
                    m.after_state,
                ))
            };
            let _ = tx.send(res);
        });
    }

    /// 后台探测社区包的链接可用性，避免给用户一个点了没反应的下载按钮
    fn probe_community(&mut self) {
        if self.probe_rx.is_some() {
            return;
        }
        let refs: Vec<(String, violet::Ref)> = self
            .packages
            .iter()
            // 社区降级包在查询阶段已经逐个换过直链并读过首部，
            // 链接是否还活着当场就知道了，不必再探一遍。
            .filter(|p| p.source == Source::Community && p.vio.is_some())
            .map(|p| (p.key(), p.vio.clone().unwrap()))
            .collect();
        if refs.is_empty() {
            return;
        }
        let (tx, rx) = channel();
        self.probe_rx = Some(rx);
        std::thread::spawn(move || {
            let mut out = Vec::new();
            for (key, r) in refs {
                out.push((key, violet::probe(&r)));
            }
            let _ = tx.send(out);
        });
    }

    /// 每帧调用：收集后台线程的结果
    pub fn poll(&mut self) {
        // 进度落历史（内部有节流，不是每帧都写盘）
        self.sync_history();

        // 「正在暂停…」只是过渡文案，线程停稳后要换掉，
        // 否则暂停早就完成了，状态栏还一直写着「正在暂停」。
        if self.status == "正在暂停…" && !self.pausing() {
            if let Some(p) = self.progress_snapshot() {
                if p.paused {
                    let mb = p.downloaded as f64 / 1_048_576.0;
                    self.set(
                        Tone::Warn,
                        format!("已暂停（{:.0} MB 已落盘），随时可以继续下载", mb),
                    );
                }
            }
        }

        if let Some(rx) = &self.query_rx {
            match rx.try_recv() {
                Ok(Ok((code, pkgs, after, community, after_state))) => {
                    let n = pkgs.len();
                    // 官方 = 版本清单里去掉社区源补的那些；售后包单独计数，
                    // 不能混进这个口径，否则「官方 N 个」会把降级包也数进去。
                    let official = pkgs.iter().filter(|p| p.source == Source::Official).count();
                    self.packages = pkgs;
                    self.aftersale = after;
                    self.after_state = after_state;
                    self.community_ok = community;
                    if let Some(m) = &mut self.current {
                        if m.name == code {
                            m.name = code.clone();
                        }
                    }
                    self.querying = false;
                    self.query_rx = None;
                    // 售后那段结论单独拼：空着不等于「没有」，必须把「没查通」说清楚，
                    // 否则用户只会看到售后分区不见了，以为程序坏了。
                    let extra = self.aftersale.len();
                    let tail = if extra > 0 {
                        format!("；另有 {} 个售后全量包", extra)
                    } else {
                        match after_state {
                            ota::AfterState::NoProject => {
                                "；该机型的官方数据里没有售后全量包".to_string()
                            }
                            ota::AfterState::Unknown => {
                                "；售后包这次没查到，可点「重新扫描」重试".to_string()
                            }
                            ota::AfterState::Ok => String::new(),
                        }
                    };
                    if community && n > official {
                        self.set(
                            Tone::Ok,
                            format!(
                                "找到 {} 个版本：官方 {} 个，社区源补 {} 个{}",
                                n,
                                official,
                                n - official,
                                tail
                            ),
                        );
                    } else {
                        // 售后一个都没有时语气收一档：这句补充说明比「找到了 N 个」更值得看
                        let tone = if extra == 0 && after_state == ota::AfterState::Unknown {
                            Tone::Warn
                        } else {
                            Tone::Ok
                        };
                        self.set(tone, format!("找到 {} 个可下载的全量包{}", n, tail));
                    }
                    self.probe_community();
                }
                Ok(Err(e)) => {
                    self.querying = false;
                    self.query_rx = None;
                    self.packages.clear();
                    self.aftersale.clear();
                    self.set(Tone::Err, e);
                }
                Err(TryRecvError::Disconnected) => {
                    self.querying = false;
                    self.query_rx = None;
                }
                Err(TryRecvError::Empty) => {}
            }
        }

        if let Some(rx) = &self.probe_rx {
            match rx.try_recv() {
                Ok(list) => {
                    for (key, size) in list {
                        for p in &mut self.packages {
                            if p.key() == key {
                                p.link_ok = Some(size.is_some());
                                if let Some(n) = size {
                                    if p.size == 0 {
                                        p.size = n;
                                    }
                                }
                            }
                        }
                    }
                    self.probe_rx = None;
                }
                Err(TryRecvError::Disconnected) => self.probe_rx = None,
                Err(TryRecvError::Empty) => {}
            }
        }

        if let Some(rx) = &self.size_rx {
            match rx.try_recv() {
                Ok((key, res)) => {
                    self.size_rx = None;
                    let Some(pkg) = self.pending.take() else {
                        return;
                    };
                    let pkg = match res {
                        Ok(n) => Package { size: n, ..pkg },
                        Err(e) => {
                            self.set(Tone::Err, format!("社区源换链接失败：{}", e));
                            return;
                        }
                    };
                    if pkg.key() != key {
                        return;
                    }
                    self.begin(&pkg);
                }
                Err(TryRecvError::Disconnected) => self.size_rx = None,
                Err(TryRecvError::Empty) => {}
            }
        }

        if self.resolving {
            if let Some(rx) = &self.link_rx {
                match rx.try_recv() {
                    Ok(Ok((url, exp))) => {
                        let (id, _ver) = self.link_for.clone().unwrap_or_default();
                        self.pending_copy = Some(url.clone());
                        let source = self
                            .packages
                            .iter()
                            .chain(self.aftersale.iter())
                            .find(|p| p.id() == id)
                            .map(|p| p.source)
                            .unwrap_or(Source::Official);
                        self.link = Some(LinkInfo {
                            url,
                            expires: exp,
                            id,
                            source,
                        });
                        self.resolving = false;
                        self.link_rx = None;
                        self.set(
                            Tone::Ok,
                            match source {
                                Source::Community => {
                                    "社区源直链已复制（作者 Smart-Paocai），约 10 分钟内有效"
                                }
                                Source::AfterCommunity => {
                                    // 社区售后这一路混着两种链：售后通道给的是长期裸链，
                                    // 常规清单里的回退包给的是 10 分钟签名链。统一不说有效期，
                                    // 只说「失效再点一次」，两种情况都不会讲错。
                                    "社区收录的售后直链已复制（作者 Smart-Paocai），失效就再点一次"
                                }
                                Source::AfterSale => {
                                    "售后直链已复制，长期有效、支持断点续传"
                                }
                                Source::Official => "官方直链已获取，约 10 分钟内有效",
                            },
                        );
                    }
                    Ok(Err(e)) => {
                        self.resolving = false;
                        self.link_rx = None;
                        self.set(Tone::Err, format!("解析直链失败：{}", e));
                    }
                    Err(TryRecvError::Disconnected) => {
                        self.resolving = false;
                        self.link_rx = None;
                    }
                    Err(TryRecvError::Empty) => {}
                }
            } else {
                self.resolving = false;
            }
        }
    }

    /// 解析一个包的直接下载地址（官方走 302 换签，社区源走它的换链接口）
    pub fn resolve_link(&mut self, pkg: &Package) {
        if self.resolving {
            return;
        }
        self.resolving = true;
        self.link = None;
        self.link_for = Some((pkg.id(), pkg.version.clone()));
        self.set(
            Tone::Info,
            match pkg.source {
                Source::Community | Source::AfterCommunity => "正在向社区源换取下载直链…",
                _ => "正在向官方服务器解析直链…",
            },
        );
        let manual = pkg.manual_url.clone();
        let vio = pkg.vio.clone();
        let (tx, rx) = channel();
        self.link_rx = Some(rx);
        std::thread::spawn(move || {
            let res = match vio {
                Some(r) => violet::download_link(&r)
                    .map(|u| {
                        let exp = ota::parse_expires(&u);
                        (u, exp)
                    }),
                None => ota::resolve_direct_url(&manual),
            };
            let _ = tx.send(res);
        });
    }

    /// 开始下载。社区包还不知道文件大小时先问出来，再落到 [\`App::begin\`]。
    pub fn start_download(&mut self, pkg: &Package) {
        if self.is_downloading() {
            self.set(Tone::Warn, "已有下载任务在进行，请等待完成或先停止");
            return;
        }
        if pkg.size == 0 {
            if let Some(r) = &pkg.vio {
                let r = r.clone();
                let key = pkg.key();
                self.pending = Some(pkg.clone());
                self.set(Tone::Info, "正在向社区源确认文件大小…");
                let (tx, rx) = channel();
                self.size_rx = Some(rx);
                std::thread::spawn(move || {
                    let _ = tx.send((key, violet::size_of(&r)));
                });
                return;
            }
            self.set(Tone::Warn, "这个版本没有可用的下载地址");
            return;
        }
        self.begin(pkg);
    }

    fn begin(&mut self, pkg: &Package) {
        let name = self.file_name_for(pkg);
        let target = PathBuf::from(&self.save_dir).join(&name);
        self.begin_to(pkg, target, name);
    }

    /// 从历史记录续传：文件还留在原来的位置，不能按保存目录重新拼路径
    pub fn resume_from_history(&mut self, entry: history::Entry) {
        if self.is_downloading() {
            self.set(Tone::Warn, "已有下载任务在进行，请等待完成或先停止");
            return;
        }
        let target = PathBuf::from(&entry.target);
        let name = target
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| self.file_name_for(&entry.pkg));
        self.history_open = false;
        self.set(Tone::Info, format!("继续下载 {}", name));
        self.begin_to(&entry.pkg, target, name);
    }

    /// 卡片上显示的文件名。
    ///
    /// 售后包早先只能拿到「ColorOS16.1.0」这种通用名，不同机型的降级包会在
    /// 下载目录里撞名，所以补机型代号当命名空间。现在版本号已经能从包内
    /// metadata 读出真名（RMX3888_16.0.10.500(CN01)，自带机型前缀），再补一层
    /// 就成了 RMX3888_RMX3888_...；所以只有版本号里还不含机型代号时才补。
    fn file_name_for(&self, pkg: &Package) -> String {
        let base = sanitize(&pkg.version);
        if matches!(pkg.source, Source::AfterSale | Source::AfterCommunity) {
            // 售后包和官方升级包可能是同一个 version_name（实测 RMX3888 两者都是
            // RMX3888_16.0.10.500(CN01)），但体积、MD5 完全不同（10.24 GB vs 8.76 GB）。
            // 同名会撞在同一个 .partstate 上，续传直接串块，必须从文件名上区分开。
            //
            // 社区收录的降级包同理：它的版本号也可能与清单里某个普通版本撞上
            // （实测 RMX3706_14.0.0.903(CN01) 就有两个文件），一样要加后缀。
            return format!("{}_售后.zip", base);
        }
        format!("{}.zip", base)
    }

    fn begin_to(&mut self, pkg: &Package, target: PathBuf, name: String) {
        let manual = pkg.manual_url.clone();
        let vio = pkg.vio.clone();
        let getter: download::UrlGetter = Arc::new(move || match &vio {
            Some(r) => violet::download_link(r),
            None => ota::resolve_direct_url(&manual).map(|(u, _)| u),
        });
        let md5 = if pkg.md5.len() == 32 {
            Some(pkg.md5.clone())
        } else {
            None
        };
        // 先落一条历史再起线程：万一程序被强杀，下次启动至少知道有这么一个任务
        self.remember(pkg, &target, history::Status::Downloading, 0);
        let h = download::start(getter, target.clone(), pkg.size, self.threads, md5);
        self.set(Tone::Info, format!("开始下载 {}", name));
        self.download = Some(ActiveDownload {
            progress: h.progress,
            cancel: h.cancel,
            name,
            target,
            pkg: pkg.clone(),
        });
    }

    /// 写一条历史记录（同一路径覆盖），并重置写盘戳
    fn remember(&mut self, pkg: &Package, target: &std::path::Path, status: history::Status, bytes: u64) {
        self.hist_stamp = Some((target.display().to_string(), status_code(status), 0));
        self.history.upsert(history::Entry {
            version: pkg.version.clone(),
            model: self
                .current
                .as_ref()
                .map(|m| m.code.clone())
                .unwrap_or_default(),
            model_name: self
                .current
                .as_ref()
                .map(|m| m.name.clone())
                .unwrap_or_default(),
            pkg: pkg.clone(),
            target: target.display().to_string(),
            status,
            bytes,
            total: pkg.size,
            updated: 0, // 由 Store::upsert 统一盖时间戳
            md5_ok: None,
        });
    }

    /// 把当前下载进度同步进历史记录。
    ///
    /// 每帧都会调，但只有「状态变了」或「进度又前进了 32MB」时才真的写盘：
    /// 全量包要下十几分钟，每帧写一次 JSON 是纯浪费，而写得太稀又会在
    /// 强杀时丢掉太多进度。32MB 大约相当于 3 秒的下载量。
    fn sync_history(&mut self) {
        let Some(d) = &self.download else { return };
        let Some(p) = self.progress_snapshot() else { return };
        let target = d.target.display().to_string();
        let pkg = d.pkg.clone();
        // 这里必须如实区分「还在下」和「已停」：以前 active 也写成 Paused，
        // 于是正在下载的任务在历史列表里挂着「已暂停」的角标，看着像坏了。
        let status = if p.done {
            history::Status::Done
        } else if p.error.is_some() {
            history::Status::Failed
        } else if p.active {
            history::Status::Downloading
        } else {
            history::Status::Paused
        };
        let code = status_code(status);
        let step = p.downloaded / (32 * 1024 * 1024);
        if let Some((t, c, s)) = &self.hist_stamp {
            if t == &target && *c == code && *s == step {
                return;
            }
        }
        self.hist_stamp = Some((target.clone(), code, step));
        let (bytes, total) = (p.downloaded, if p.total > 0 { p.total } else { pkg.size });
        let md5_ok = p.verified;
        self.history.upsert(history::Entry {
            version: pkg.version.clone(),
            model: self
                .current
                .as_ref()
                .map(|m| m.code.clone())
                .unwrap_or_default(),
            model_name: self
                .current
                .as_ref()
                .map(|m| m.name.clone())
                .unwrap_or_default(),
            pkg,
            target,
            status,
            bytes,
            total,
            updated: 0,
            md5_ok,
        });
    }

    /// 暂停：写取消标志，下载线程会在 100ms 内退出并把已完成分块落盘。
    pub fn stop_download(&mut self) {
        if let Some(d) = &self.download {
            if d.cancel.swap(true, Ordering::SeqCst) {
                return; // 已经在停了，别重复提示
            }
            self.set(Tone::Warn, "正在暂停…");
        }
    }

    /// 暂停 / 出错的任务能不能继续：有进度、没下完、线程也已经停下。
    pub fn can_resume(&self) -> bool {
        match self.progress_snapshot() {
            Some(p) => !p.active && !p.done && p.total > 0,
            None => false,
        }
    }

    /// 继续下载：用同一个包重新起一次。
    ///
    /// [`download::start`] 会先读 .partstate 里的分块位图，跳过已经下完的块，
    /// 所以这里不需要自己算偏移，也不会重复占磁盘。
    pub fn resume_download(&mut self) {
        let Some(d) = &self.download else { return };
        // progress.active 只有下载线程真正退出时才会变 false，
        // 所以它同时代表"还在跑"和"暂停还没停干净"，用它一个判断就够了。
        // 不能去看 cancel 标志：暂停后它是常驻 true 的。
        if self.is_downloading() {
            return;
        }
        if !self.can_resume() {
            self.set(Tone::Warn, "这个任务没有可继续的进度，请重新下载");
            return;
        }
        // ★ 必须沿用任务原来的 target，不能拿 pkg 重新拼一次路径。
        // 走过的坑：以前这里是 begin(&pkg)，而 begin() 会用当前「保存到」目录 +
        // file_name_for(pkg) 重新组路径。只要用户中途改过保存目录（或者当前机型
        // 不是这个包所属的机型，售后包的机型前缀就会拼错），新路径就和
        // 已有的 .partstate 对不上 —— 位图读不到，续传静默变成从头下载，
        // 8 GB 的文件等于白下。resume_from_history 一直是对的，这里对齐它。
        let pkg = d.pkg.clone();
        let target = d.target.clone();
        let name = d.name.clone();
        self.set(Tone::Info, "继续下载，已完成的块会跳过");
        self.begin_to(&pkg, target, name);
    }

    /// 把任务从底栏移除（文件留在磁盘上）。只要求线程已经停下。
    pub fn clear_finished(&mut self) {
        if self.is_downloading() {
            return;
        }
        self.download = None;
        self.set(Tone::Info, "任务已从底栏移除，文件仍在保存目录里");
    }

    /// 已经点了暂停、但下载线程还没退干净。
    ///
    /// 这段时间界面必须继续刷新，否则「暂停中」会一直卡在屏幕上——
    /// 线程把 active 置 false 的那一刻，egui 并不知道要重画。
    pub fn pausing(&self) -> bool {
        match &self.download {
            Some(d) => {
                d.cancel.load(Ordering::SeqCst)
                    && d.progress.lock().map(|p| p.active).unwrap_or(false)
            }
            None => false,
        }
    }

    /// 这个包是不是"暂停在手上"的（卡片按钮文案用）
    pub fn is_paused_pkg(&self, pkg: &Package) -> bool {
        match &self.download {
            Some(d) => d.pkg.id() == pkg.id() && !self.is_downloading() && self.can_resume(),
            None => false,
        }
    }

    /// 这个包是否正在下载（卡片按钮用，避免所有卡片一起变成"下载中"）
    ///
    /// 比的是 Package::id 而不是文件名：售后包的版本号是通用名，
    /// 拿「ColorOS16.1.0」去比对，会让别的机型同版本号的卡片一起变灰。
    pub fn is_downloading_pkg(&self, pkg: &Package) -> bool {
        match &self.download {
            Some(d) => self.is_downloading() && d.pkg.id() == pkg.id(),
            None => false,
        }
    }

    pub fn is_downloading(&self) -> bool {
        self.download
            .as_ref()
            .and_then(|d| d.progress.lock().ok().map(|p| p.active))
            .unwrap_or(false)
    }

    /// 读取快照，避免 UI 层反复加锁
    pub fn progress_snapshot(&self) -> Option<Progress> {
        self.download
            .as_ref()
            .and_then(|d| d.progress.lock().ok().map(|p| p.clone()))
    }

    /// 在资源管理器里定位当前下载的文件。
    ///
    /// 两条走过的弯路，别再用：
    /// 1. open crate：先试 powershell 的 Invoke-Item，对文件只会调默认关联程序
    ///    （.zip 变成打开压缩包查看器），而且是同步等待，界面会卡。
    /// 2. std::process::Command 直接起 explorer.exe：**看起来成功，其实什么都不会弹**。
    ///    explorer.exe 被直接 spawn 时初始化失败，退出码 0xC0000142
    ///    （STATUS_DLL_INIT_FAILED），但 spawn() 本身返回 Ok。
    ///    现在统一走 `SHOpenFolderAndSelectItems` 与 Win10/Win11 Shell 回退；
    ///    若明确返回拒绝访问，只在本次定位操作请求一次 UAC helper。
    /// 定位任意已下载文件（历史记录里用）。
    ///
    /// 与 reveal_download 同一套策略：文件在就选中它，不在就打开所在目录，
    /// 路径一律先放剪贴板兜底。
    pub fn reveal_path(&mut self, file: &str) {
        let path = PathBuf::from(file);
        let dir = path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from(&self.save_dir));
        let has_file = path.is_file();
        let target = if has_file {
            path.display().to_string()
        } else {
            dir.display().to_string()
        };
        self.pending_copy = Some(target.clone());
        match crate::app::shell::reveal(&target, has_file) {
            Ok(()) => self.set(
                Tone::Ok,
                format!("已在资源管理器中定位（路径同时复制到了剪贴板）：{}", target),
            ),
            Err(hr) => self.set(
                Tone::Err,
                format!(
                    "资源管理器没有响应（外壳错误码 0x{:08X}），路径已复制到剪贴板：{}",
                    hr, target
                ),
            ),
        }
    }

    pub fn reveal_download(&mut self) {
        let Some(d) = &self.download else {
            self.set(Tone::Warn, "还没有下载任务");
            return;
        };
        let file = d.target.clone();
        let dir = file
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from(&self.save_dir));

        let has_file = file.is_file();
        let target = if has_file {
            file.display().to_string()
        } else {
            dir.display().to_string()
        };

        // 兜底：不管资源管理器有没有被拉起来，都把路径放进剪贴板。
        // 有些环境（远程桌面、受策略限制的账户、被安全软件拦住的外壳调用）
        // 会返回成功码但窗口不出现，剪贴板至少让用户能立刻粘贴到地址栏。
        self.pending_copy = Some(target.clone());

        match crate::app::shell::reveal(&target, has_file) {
            Ok(()) => self.set(
                Tone::Ok,
                format!("已在资源管理器中定位（路径同时复制到了剪贴板）：{}", target),
            ),
            Err(hr) => self.set(
                Tone::Err,
                format!(
                    "资源管理器没有响应（外壳错误码 0x{:08X}），路径已复制到剪贴板：{}",
                    hr, target
                ),
            ),
        }
    }

    /// 直链剩余秒数
    pub fn link_left(&self) -> Option<i64> {
        let l = self.link.as_ref()?;
        if l.expires <= 0 {
            return None;
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        Some(l.expires - now)
    }
}

/// 历史状态压成一个小整数，用于判断「这次是否值得写盘」
fn status_code(s: history::Status) -> u8 {
    match s {
        history::Status::Paused => 0,
        history::Status::Done => 1,
        history::Status::Failed => 2,
        history::Status::Downloading => 3,
    }
}

/// 把版本号变成安全的文件名
pub fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '.' | '_' | '-' | '(' | ')') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// 默认保存目录：用户下载文件夹
///
/// 必须用 PathBuf::join 而不是手拼 format!("{}/Downloads", ...)：
/// 拼出来是 C:\Users\GuLi/Downloads 这种半截反斜杠的混合路径，
/// 在资源管理器地址栏、以及复制到剪贴板给用户时都很难看。
pub fn default_dir() -> String {
    if let Ok(profile) = std::env::var("USERPROFILE") {
        let base = std::path::PathBuf::from(&profile);
        let dl = base.join("Downloads");
        if dl.is_dir() {
            return dl.display().to_string();
        }
        return base.display().to_string();
    }
    ".".to_string()
}

