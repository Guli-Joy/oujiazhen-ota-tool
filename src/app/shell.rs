//! 在资源管理器里定位文件（Windows 外壳调用）
//!
//! 走过的弯路，全都别再用 —— 每一条都是实测踩出来的：
//!
//! 1. open crate：内部调 powershell 的 Invoke-Item，对文件只会调默认关联程序
//!    （.zip 直接变成打开压缩包查看器），而且是同步等待，界面会卡好几秒。
//!
//! 2. std::process::Command::new("explorer.exe")：spawn() 返回 Ok，
//!    但子进程以 0xC0000142（STATUS_DLL_INIT_FAILED）退出，一个窗口都不会出现。
//!
//! 3. ShellExecuteW(NULL, "open", "explorer.exe", "/select,<path>")：
//!    返回 42（> 32 即被当成成功），可实际弹出来的是
//!    「explorer.exe - Application Error / 应用程序无法正常启动(0x0000142)」错误框。
//!    原因是从别的进程里再 spawn 一个 explorer.exe 时，新进程继承本进程的环境块
//!    （PATH 里混着 MSYS 路径、TMP 也被改过），explorer 的 DLL 初始化当场崩掉。
//!    返回值只说明外壳命令发出去了，根本不代表窗口出现 —— 这才是之前
//!    「点了打开文件夹没反应」的真正原因：它一直在弹错误框，只是没人往那儿看。
//!
//! 现在用的是 SHOpenFolderAndSelectItems：外壳自己推荐的「打开文件夹并选中某项」
//! 接口。它不创建任何进程，只是通过 COM 通知已经在跑的资源管理器去定位，
//! 所以既不会崩，也不会被前台策略压到程序后面。文件不在了就退回打开所在目录。

use std::ffi::{c_void, OsStr};
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

#[link(name = "shell32")]
extern "system" {
    fn ShellExecuteW(
        hwnd: *mut c_void,
        op: *const u16,
        file: *const u16,
        params: *const u16,
        dir: *const u16,
        show: i32,
    ) -> isize;
    fn ILCreateFromPathW(psz_path: *const u16) -> *mut c_void;
    fn ILFree(pidl: *mut c_void);
    fn SHOpenFolderAndSelectItems(
        pidl_folder: *const c_void,
        cidl: u32,
        apidl: *const *const c_void,
        flags: u32,
    ) -> i32;
}

#[link(name = "ole32")]
extern "system" {
    fn CoInitializeEx(reserved: *mut c_void, co_init: u32) -> i32;
}

#[link(name = "user32")]
extern "system" {
    fn AllowSetForegroundWindow(dw_process_id: u32);
}

/// ASFW_ANY：把「可以抢前台」的许可发给接下来要响应请求的外壳
const ASFW_ANY: u32 = -1i32 as u32;
/// COINIT_APARTMENTTHREADED：外壳对象要求单线程套间
const COINIT_APARTMENTTHREADED: u32 = 0x2;

fn wide(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}

/// 让 pidl 指向的项在资源管理器里被选中（pidl 是文件夹时就打开它自己）。
///
/// 返回 HRESULT：0 表示外壳确实接下了这个请求。非 0 时调用方会退回打开目录，
/// 并把码放进提示里方便排查（远程会话、被安全软件拦住的账户都可能返回别的值）。
fn select_pidl(pidl: *mut c_void) -> i32 {
    if pidl.is_null() {
        // ILCreateFromPathW 失败（路径不存在 / 不是绝对路径）
        return -0x7FFF_FFFF;
    }
    let hr = unsafe { SHOpenFolderAndSelectItems(pidl, 0, std::ptr::null(), 0) };
    unsafe { ILFree(pidl) };
    hr
}

fn pidl_of(path: &str) -> *mut c_void {
    let w = wide(path);
    unsafe { ILCreateFromPathW(w.as_ptr()) }
}

/// 打开资源管理器并定位 path：文件就选中它，目录就打开它。
///
/// 成功返回 Ok(())，失败返回最后一次外壳调用的 HRESULT（十六进制方便对照文档）。
/// 把错误码带出去是刻意的：这类调用失败时系统只给一个数字，没有它根本没法排查。
///
/// 为什么必须另起线程：SHOpenFolderAndSelectItems 是 COM 调用，要求调用线程是
/// 单线程套间（STA）。而 egui 的主线程由 winit 建出来，套间模型不受我们控制
/// （实测主线程上调它一律失败，退回 ShellExecuteW 又拿到 5 = 拒绝访问）。
/// 起一个专门的新线程，先 CoInitializeEx(APARTMENTTHREADED) 再调用，就一定在 STA 里。
pub fn reveal(path: &str, select: bool) -> Result<(), i64> {
    let path = path.to_string();
    let (tx, rx) = std::sync::mpsc::channel();
    // 线程起不来（极端资源耗尽）就当外壳调用失败，由调用方退回剪贴板
    if std::thread::Builder::new()
        .name("shell-reveal".into())
        .spawn(move || {
            let _ = tx.send(reveal_sta(&path, select));
        })
        .is_err()
    {
        return Err(-1);
    }
    // 外壳正常是毫秒级返回；卡住就不等了，界面不能因为点个按钮而僵住
    rx.recv_timeout(std::time::Duration::from_secs(3))
        .unwrap_or(Err(-2))
}

/// 真正干活的 STA 版本（只在 reveal 起的那个线程里调用）
fn reveal_sta(path: &str, select: bool) -> Result<(), i64> {
    let mut log = String::new();
    let hr_init = unsafe { CoInitializeEx(std::ptr::null_mut(), COINIT_APARTMENTTHREADED) };
    unsafe { AllowSetForegroundWindow(ASFW_ANY) };
    let env = |k: &str| std::env::var(k).unwrap_or_else(|_| "<missing>".into());
    log.push_str(&format!(
        "CoInit=0x{hr_init:08X} path={path} exists={}\n  ENV SystemRoot={} windir={} USERPROFILE={} SESSIONNAME={} TEMP={}\n",
        Path::new(path).is_file(),
        env("SystemRoot"),
        env("windir"),
        env("USERPROFILE"),
        env("SESSIONNAME"),
        env("TEMP"),
    ));

    let mut last: i64 = 0;
    if select {
        let pidl = pidl_of(path);
        let hr = select_pidl(pidl);
        log.push_str(&format!("select file: pidl_null={} hr=0x{:08X}\n", pidl.is_null(), hr));
        if hr == 0 {
            flush_log(&log);
            return Ok(());
        }
        last = hr as i64;
    }

    // 走到这里说明「选中某个文件」没成功（多半是文件已经不在了），
    // 退一步直接打开所在目录。目录项同样走这个接口，不 spawn 进程。
    if let Some(dir) = Path::new(path).parent().map(|p| p.display().to_string()) {
        let pidl = pidl_of(&dir);
        let hr = select_pidl(pidl);
        log.push_str(&format!("select dir [{dir}]: pidl_null={} hr=0x{:08X}\n", pidl.is_null(), hr));
        if hr == 0 {
            flush_log(&log);
            return Ok(());
        }
        last = hr as i64;
    }

    // 最后一条路：让外壳按默认动词打开。注意传的是**路径**而不是 explorer.exe ——
    // 后者正是第 3 条踩过的 0x0142 崩溃。先试所在目录（用户要的就是"看看文件在哪"），
    // 目录也打不开才试文件本身。
    let dir_try = Path::new(path).parent().map(|p| p.display().to_string());
    let mut rets: Vec<(&str, isize)> = Vec::new();
    for (label, target) in [
        ("dir", dir_try.clone()),
        ("file", Some(path.to_string())),
    ] {
        let Some(t) = target else { continue };
        let op = wide("open");
        let w = wide(&t);
        let ret = unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                op.as_ptr(),
                w.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                1, // SW_SHOWNORMAL
            )
        };
        rets.push((label, ret));
        if ret > 32 {
            log.push_str(&format!("ShellExecuteW({label}) ret={ret} OK\n"));
            flush_log(&log);
            return Ok(());
        }
    }
    // 再退一步：经典的 explorer.exe /select 写法。它在本机能起来的桌面会话里
    // 通常有效（前面踩到的 0x0142 只在"进程环境块被污染"的场合出现），
    // 所以留作最后一条路，而不是当主路径。
    let op = wide("open");
    let exe = wide("explorer.exe");
    let args = wide(&format!("/select,\"{}\"", path));
    let ret = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            op.as_ptr(),
            exe.as_ptr(),
            args.as_ptr(),
            std::ptr::null(),
            1,
        )
    };
    rets.push(("explorer-select", ret));
    log.push_str(&format!("ShellExecuteW results: {rets:?}\n"));
    flush_log(&log);
    if ret > 32 {
        Ok(())
    } else {
        Err(if last != 0 {
            last
        } else {
            rets.first().map(|r| r.1 as i64).unwrap_or(-3)
        })
    }
}

/// 把排查信息追加到 %TEMP%\ota-tool-shell.log（只在开发期看得见，正式版没有副作）
fn flush_log(s: &str) {
    if let Ok(tmp) = std::env::var("TEMP") {
        let p = Path::new(&tmp).join("ota-tool-shell.log");
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
            use std::io::Write;
            let _ = f.write_all(s.as_bytes());
        }
    }
}
