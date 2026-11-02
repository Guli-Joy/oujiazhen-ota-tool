// 多线程分块下载器：共享连接池 + 断点续传 + MD5 校验
use md5::{Digest, Md5};
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const CHUNK: u64 = 8 * 1024 * 1024;

#[derive(Clone, Debug, Default)]
pub struct Progress {
    pub downloaded: u64,
    pub total: u64,
    pub speed_bps: f64,
    pub done: bool,
    pub error: Option<String>,
    pub eta_secs: f64,
    pub active: bool,
    pub verifying: bool,
    pub verified: Option<bool>,
    /// 被用户主动暂停（区别于出错停止）：界面上要显示「继续下载」而不是报错
    pub paused: bool,
}

impl Progress {
    pub fn percent(&self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            (self.downloaded as f64 / self.total as f64 * 100.0) as f32
        }
    }
    pub fn speed_text(&self) -> String {
        let s = self.speed_bps;
        if s <= 0.0 {
            "-".into()
        } else if s >= 1_048_576.0 {
            format!("{:.1} MB/s", s / 1_048_576.0)
        } else if s >= 1024.0 {
            format!("{:.0} KB/s", s / 1024.0)
        } else {
            format!("{:.0} B/s", s)
        }
    }
    pub fn eta_text(&self) -> String {
        if self.eta_secs <= 0.0 {
            "-".into()
        } else {
            let t = self.eta_secs as u64;
            if t >= 3600 {
                format!("{} 时 {} 分", t / 3600, (t % 3600) / 60)
            } else if t >= 60 {
                format!("{} 分 {} 秒", t / 60, t % 60)
            } else {
                format!("{} 秒", t)
            }
        }
    }
}

pub type UrlGetter = Arc<dyn Fn() -> Result<String, String> + Send + Sync>;

pub struct DownloadHandle {
    pub progress: Arc<Mutex<Progress>>,
    pub cancel: Arc<AtomicBool>,
}

/// 点暂停后最多再等这么久，超过就直接落盘返回。
///
/// 为什么需要：取消标志只能让线程在「下一个 64KB 读」时退出，而阻塞式读最长要
/// 等到 socket 超时。原来主线程老老实实 join 所有线程，用户就会看到进度条继续
/// 往上走十几秒 —— 表现就是「点了没反应，要点好几次」。
/// 到点后不再等待是安全的：剩下的线程各自最多再读完一个数据块就退出，它们补完的
/// 块没有被记进 .partstate 位图，下次续传会重下那几块（每块写在固定偏移上，整体
/// 覆盖），不会留下任何坏数据。
const STOP_GRACE: Duration = Duration::from_millis(900);

/// 建连超时：CDN 正常是毫秒级握手，10 秒足够覆盖抖动
const TIMEOUT_CONNECT: Duration = Duration::from_secs(10);
/// 单次读超时（不是整块总时长）：按 64KB 一次读，即使只有 100KB/s 也只要 0.6 秒，
/// 8 秒已经非常宽松。原来是 25 秒，那正是「暂停要等很久」的最大来源。
const TIMEOUT_READ: Duration = Duration::from_secs(8);
/// 写超时：本地写盘，给足余量防止慢盘误判
const TIMEOUT_WRITE: Duration = Duration::from_secs(30);

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(TIMEOUT_CONNECT)
        .timeout_read(TIMEOUT_READ)
        .timeout_write(TIMEOUT_WRITE)
        .build()
}

pub fn start(
    url_getter: UrlGetter,
    target: PathBuf,
    total: u64,
    threads: usize,
    expect_md5: Option<String>,
) -> DownloadHandle {
    let progress = Arc::new(Mutex::new(Progress {
        total,
        active: true,
        ..Default::default()
    }));
    let cancel = Arc::new(AtomicBool::new(false));
    let handle = DownloadHandle {
        progress: progress.clone(),
        cancel: cancel.clone(),
    };

    std::thread::spawn(move || {
        let result = run(
            url_getter,
            &target,
            total,
            threads,
            &progress,
            &cancel,
            expect_md5.as_deref(),
        );
        if let Ok(mut p) = progress.lock() {
            p.active = false;
            p.verifying = false;
            match result {
                // 暂停也走 Ok 分支，靠 paused 标志区分：不能算完成
                Ok(()) => {
                    if !p.paused {
                        p.done = true;
                        p.error = None;
                    }
                }
                Err(e) => {
                    p.error = Some(e);
                }
            }
        }
    });
    handle
}

pub fn state_path(target: &Path) -> PathBuf {
    let mut p = target.to_path_buf();
    let name = format!(
        "{}.partstate",
        p.file_name().and_then(|s| s.to_str()).unwrap_or("dl")
    );
    p.set_file_name(name);
    p
}

fn load_state(target: &Path) -> Option<(u64, Vec<bool>)> {
    let txt = std::fs::read_to_string(state_path(target)).ok()?;
    let mut lines = txt.lines();
    let total: u64 = lines.next()?.trim().parse().ok()?;
    let bits = lines.next()?.trim().to_string();
    Some((total, bits.chars().map(|c| c == '1').collect()))
}

fn save_state(target: &Path, total: u64, done: &[bool]) {
    let line: String = done.iter().map(|&b| if b { '1' } else { '0' }).collect();
    let _ = std::fs::write(state_path(target), format!("{}\n{}\n", total, line));
}

fn md5_of(path: &Path) -> Option<String> {
    let mut f = std::fs::File::open(path).ok()?;
    let mut hasher = Md5::new();
    let mut buf = vec![0u8; 4 * 1024 * 1024];
    loop {
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Some(format!("{:x}", hasher.finalize()))
}

fn run(
    url_getter: UrlGetter,
    target: &PathBuf,
    total: u64,
    threads: usize,
    progress: &Arc<Mutex<Progress>>,
    cancel: &Arc<AtomicBool>,
    expect_md5: Option<&str>,
) -> Result<(), String> {
    if total == 0 {
        return Err("文件大小为 0，无法下载".into());
    }
    let n_chunks = ((total + CHUNK - 1) / CHUNK) as usize;
    let done = match load_state(target) {
        Some((t, d)) if t == total && d.len() == n_chunks => d,
        _ => vec![false; n_chunks],
    };

    // 目标目录可能已被用户改名/删除（例如保存目录指向一个临时文件夹），
    // 这里补建一次，失败时把完整路径报出来，避免只看到一句「系统找不到指定的路径」。
    if let Some(parent) = target.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| {
                format!(
                    "无法创建保存目录 {}：{}",
                    parent.display(),
                    e
                )
            })?;
        }
    }
    {
        let f = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(target)
            .map_err(|e| format!("无法创建文件 {}：{}", target.display(), e))?;
        if f.metadata().map(|m| m.len()).unwrap_or(0) != total {
            f.set_len(total)
                .map_err(|e| format!("无法预分配磁盘空间（需要 {:.2} GB）：{}", total as f64 / 1.073_741_824e9, e))?;
        }
    }

    let initial: u64 = done
        .iter()
        .enumerate()
        .filter(|(_, &d)| d)
        .map(|(i, _)| (total - i as u64 * CHUNK).min(CHUNK))
        .sum();

    // 已全部下载完：直接进入校验
    if done.iter().all(|&b| b) {
        return verify(target, expect_md5, progress);
    }

    let file = Arc::new(Mutex::new(
        OpenOptions::new()
            .write(true)
            .open(target)
            .map_err(|e| e.to_string())?,
    ));
    let next = Arc::new(AtomicU64::new(0));
    // 平滑进度：按实际收到字节累加，不是按块完成才跳
    let downloaded = Arc::new(AtomicU64::new(initial));
    // 收尾闸门：主线程把最终进度定稿后置 true，还没退出的线程从此不许再动
    // 计数与进度（否则它们的 fetch_sub 会让数字回跳甚至下溢成天文数字）。
    let sealed = Arc::new(AtomicBool::new(false));
    let done_flag = Arc::new(Mutex::new(done));
    let shared_agent = agent();
    let url_now = Arc::new(Mutex::new(url_getter()?));
    let first_err: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let _started = Instant::now();

    if let Ok(mut p) = progress.lock() {
        p.downloaded = initial;
        p.total = total;
    }

    // 独立测速线程：每 400ms 用滑动窗口算一次瞬时速度，UI 才不会一格一格跳
    let speed_stop = Arc::new(AtomicBool::new(false));
    {
        let speed_stop = speed_stop.clone();
        let downloaded = downloaded.clone();
        let progress = progress.clone();
        let speed_state = Arc::new(Mutex::new(SpeedWindow::new(initial)));
        let speed_state2 = speed_state.clone();
        std::thread::spawn(move || {
            while !speed_stop.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(400));
                let cur = downloaded.load(Ordering::Relaxed);
                let sp = match speed_state2.lock() {
                    Ok(mut w) => w.push(cur),
                    Err(_) => 0.0,
                };
                if sp > 0.0 {
                    if let Ok(mut p) = progress.lock() {
                        p.speed_bps = sp;
                        let remain = total.saturating_sub(cur);
                        p.eta_secs = remain as f64 / sp;
                    }
                }
            }
        });
    }

    let mut handles = Vec::new();
    for _ in 0..threads.max(1).min(64) {
        let next = next.clone();
        let file = file.clone();
        let downloaded = downloaded.clone();
        let done_flag = done_flag.clone();
        let shared_agent = shared_agent.clone();
        let url_now = url_now.clone();
        let progress = progress.clone();
        let cancel = cancel.clone();
        let first_err = first_err.clone();
        let url_getter = url_getter.clone();
        let sealed = sealed.clone();
        let total_c = total;

        handles.push(std::thread::spawn(move || {
            loop {
                if cancel.load(Ordering::Relaxed) {
                    return;
                }
                let idx = next.fetch_add(1, Ordering::SeqCst) as usize;
                if idx >= n_chunks {
                    return;
                }
                if done_flag
                    .lock()
                    .map(|d| d.get(idx).copied().unwrap_or(false))
                    .unwrap_or(false)
                {
                    continue;
                }
                let start = idx as u64 * CHUNK;
                let end = (start + CHUNK - 1).min(total_c - 1);
                let want = end - start + 1;

                let mut ok = false;
                // 被用户停下时不再重试，也不再报「反复失败」，直接退出线程
                let mut cancelled = false;
                for attempt in 0..12u64 {
                    if cancel.load(Ordering::Relaxed) {
                        return;
                    }
                    let url = url_now.lock().map(|u| u.clone()).unwrap_or_default();
                    match fetch_range(
                        &shared_agent,
                        &url,
                        start,
                        end,
                        &downloaded,
                        &progress,
                        total_c,
                        &cancel,
                        &sealed,
                    ) {
                        Err(e) if e == CANCELLED => {
                            cancelled = true;
                            break;
                        }
                        Ok(buf) if buf.len() as u64 == want => {
                            let mut f = match file.lock() {
                                Ok(f) => f,
                                Err(_) => return,
                            };
                            if f.seek(SeekFrom::Start(start)).is_err() || f.write_all(&buf).is_err() {
                                if let Ok(mut e) = first_err.lock() {
                                    *e = Some("写入磁盘失败，可能磁盘空间不足".into());
                                }
                                return;
                            }
                            drop(f);
                            if let Ok(mut d) = done_flag.lock() {
                                if idx < d.len() {
                                    d[idx] = true;
                                }
                            }
                            ok = true;
                            break;
                        }
                        _ => {
                            // 签名过期：重新申请直链
                            if attempt >= 1 {
                                if let Ok(fresh) = url_getter() {
                                    if let Ok(mut u) = url_now.lock() {
                                        *u = fresh;
                                    }
                                }
                            }
                            // 退避期间也要能被打断：之前是整段 sleep，
                            // 最长 3.6 秒才醒，用户点暂停后会觉得按钮没反应。
                            sleep_cancellable(Duration::from_millis(300 * (attempt + 1)), &cancel);
                        }
                    }
                }
                if !ok {
                    if !cancelled {
                        if let Ok(mut e) = first_err.lock() {
                            if e.is_none() {
                                *e = Some(format!("第 {} 块反复失败，已停止", idx + 1));
                            }
                        }
                    }
                    return;
                }
            }
        }));
    }

    // 主线程周期性落地进度，避免异常退出丢进度。
    // 取消后按 50ms 粒度轮询，最多等 STOP_GRACE 就把当前位图落盘返回。
    let mut stop_at: Option<Instant> = None;
    let mut detached = false;
    loop {
        if !handles.iter().any(|h| !h.is_finished()) {
            break;
        }
        if cancel.load(Ordering::Relaxed) {
            let t0 = *stop_at.get_or_insert_with(Instant::now);
            if t0.elapsed() > STOP_GRACE {
                // 还没退干净的线程直接放手：见 STOP_GRACE 的说明
                detached = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        } else {
            std::thread::sleep(Duration::from_secs(5));
        }
        if let Ok(d) = done_flag.lock() {
            save_state(target, total, &d);
        }
    }

    // 只有正常收尾才 join：被放开的那几个线程继续跑会自己退出，
    // JoinHandle 一 drop 就 detach，不影响这里把进度定稿。
    if !detached {
        for h in handles {
            let _ = h.join();
        }
    }

    speed_stop.store(true, Ordering::Relaxed);

    // 先把闸门关上，再取快照、定稿进度：顺序反了的话，落后的线程可能在我们
    // 定稿之后又改一次 downloaded，界面上就会跳出一个假的更高进度。
    sealed.store(true, Ordering::SeqCst);
    let snapshot = done_flag.lock().map(|d| d.clone()).unwrap_or_default();
    save_state(target, total, &snapshot);

    // 已落盘的字节数：暂停/失败时把进度纠正成磁盘上真实完成量，
    // 避免显示值包含被丢弃的半截块，导致续传时数字突然倒退
    let committed: u64 = snapshot
        .iter()
        .enumerate()
        .filter(|(_, &d)| d)
        .map(|(i, _)| (total - i as u64 * CHUNK).min(CHUNK))
        .sum();
    if let Ok(mut p) = progress.lock() {
        p.downloaded = committed;
    }
    downloaded.store(committed, Ordering::Relaxed);

    if let Some(e) = first_err.lock().ok().and_then(|e| e.clone()) {
        return Err(format!("{}（进度已保存，可续传）", e));
    }
    if cancel.load(Ordering::Relaxed) {
        if let Ok(mut p) = progress.lock() {
            p.paused = true;
        }
        return Ok(());
    }
    if !snapshot.iter().all(|&b| b) {
        return Err("下载未完成，进度已保存".into());
    }

    verify(target, expect_md5, progress)
}

fn verify(
    target: &PathBuf,
    expect: Option<&str>,
    progress: &Arc<Mutex<Progress>>,
) -> Result<(), String> {
    let Some(expect) = expect.filter(|s| s.len() == 32) else {
        let _ = std::fs::remove_file(state_path(target));
        return Ok(());
    };
    if let Ok(mut p) = progress.lock() {
        p.verifying = true;
        p.downloaded = p.total;
        p.eta_secs = 0.0;
    }
    let actual = md5_of(target).unwrap_or_default();
    let matched = actual.eq_ignore_ascii_case(expect);
    if let Ok(mut p) = progress.lock() {
        p.verifying = false;
        p.verified = Some(matched);
    }
    let _ = std::fs::remove_file(state_path(target));
    if matched {
        Ok(())
    } else {
        Err(format!("MD5 校验不符（得到 {}），文件可能损坏，建议删除后重新下载", actual))
    }
}

/// 滑动窗口测速：只统计最近若干次采样，避免被启动慢/单块卡顿污染
struct SpeedWindow {
    samples: Vec<(Instant, u64)>,
}

impl SpeedWindow {
    fn new(base: u64) -> Self {
        Self {
            samples: vec![(Instant::now(), base)],
        }
    }
    fn push(&mut self, cur: u64) -> f64 {
        self.samples.push((Instant::now(), cur));
        let cutoff = Instant::now() - Duration::from_secs(6);
        self.samples.retain(|(t, _)| *t >= cutoff);
        if self.samples.len() < 2 {
            return 0.0;
        }
        let (t0, b0) = self.samples[0];
        let (t1, b1) = *self.samples.last().unwrap();
        let dt = t1.duration_since(t0).as_secs_f64();
        if dt <= 0.05 || b1 <= b0 {
            return 0.0;
        }
        (b1 - b0) as f64 / dt
    }
}

/// 取消信号专用的错误串：上层据此判断"是被用户停下，而不是网络出错"
const CANCELLED: &str = "已取消";

/// 可被取消打断的等待：切成 40ms 一段，返回 true 表示途中被取消。
fn sleep_cancellable(total: Duration, cancel: &Arc<AtomicBool>) -> bool {
    let step = Duration::from_millis(40);
    let mut left = total;
    while left > Duration::ZERO {
        if cancel.load(Ordering::Relaxed) {
            return true;
        }
        let s = left.min(step);
        std::thread::sleep(s);
        left -= s;
    }
    cancel.load(Ordering::Relaxed)
}

/// 分段请求。每读一小块就检查一次取消标志——之前只在块边界检查，
/// 用户点「停止」后要等当前 8MB 块读完 / 读超时 120 秒才真正停下，
/// 表现就是「点好几次才有反应」。
fn fetch_range(
    agent: &ureq::Agent,
    url: &str,
    start: u64,
    end: u64,
    downloaded: &Arc<AtomicU64>,
    progress: &Arc<Mutex<Progress>>,
    total: u64,
    cancel: &Arc<AtomicBool>,
    sealed: &Arc<AtomicBool>,
) -> Result<Vec<u8>, String> {
    let want = (end - start + 1) as usize;
    let resp = agent
        .get(url)
        .set("Range", &format!("bytes={}-{}", start, end))
        .set("userId", "oplus-ota|00000001")
        .call();
    match resp {
        Ok(r) => {
            let mut out = Vec::with_capacity(want.min(64 * 1024 * 1024));
            let mut reader = r.into_reader().take(want as u64);
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                if sealed.load(Ordering::SeqCst) {
                    // 主线程已经定稿，这里一个字都不能再改（连回滚都不行）
                    return Err(CANCELLED.to_string());
                }
                if cancel.load(Ordering::Relaxed) {
                    // 半截数据直接丢弃，计数回滚，交由上层把状态落成「已暂停」
                    downloaded.fetch_sub(out.len() as u64, Ordering::Relaxed);
                    return Err(CANCELLED.to_string());
                }
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if sealed.load(Ordering::SeqCst) {
                            return Err(CANCELLED.to_string());
                        }
                        out.extend_from_slice(&buf[..n]);
                        let cur = downloaded.fetch_add(n as u64, Ordering::Relaxed) + n as u64;
                        if let Ok(mut p) = progress.lock() {
                            p.downloaded = cur;
                            let remain = total.saturating_sub(cur);
                            if p.speed_bps > 1024.0 {
                                p.eta_secs = remain as f64 / p.speed_bps;
                            }
                        }
                    }
                    Err(e) => {
                        // 已收到部分数据，回退计数，交给上层重试
                        if !sealed.load(Ordering::SeqCst) {
                            downloaded.fetch_sub(out.len() as u64, Ordering::Relaxed);
                        }
                        return Err(e.to_string());
                    }
                }
            }
            if out.len() < want && !sealed.load(Ordering::SeqCst) {
                downloaded.fetch_sub(out.len() as u64, Ordering::Relaxed);
            }
            Ok(out)
        }
        Err(ureq::Error::Status(code, _)) => Err(format!("HTTP {}", code)),
        Err(e) => Err(e.to_string()),
    }
}
