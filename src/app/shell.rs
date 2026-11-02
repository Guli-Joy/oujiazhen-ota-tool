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
//! 主路径使用 Win10/Win11 都支持的 SHOpenFolderAndSelectItems；普通权限进程在
//! 定位操作前按需请求一次 runas helper，再由 helper 使用 ShellExecuteExW 打开目录。
//! 所有路径都作为参数数据传递，不拼接 explorer.exe 命令行。

use std::ffi::{c_void, OsStr, OsString};
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

#[link(name = "shell32")]
extern "system" {
    fn SHParseDisplayName(
        psz_name: *const u16,
        pbc: *mut c_void,
        ppidl: *mut *mut c_void,
        sfgao_in: u32,
        psfgao_out: *mut u32,
    ) -> i32;
    fn ILClone(pidl: *const c_void) -> *mut c_void;
    fn ILFindLastID(pidl: *const c_void) -> *mut c_void;
    fn ILRemoveLastID(pidl: *mut c_void) -> i32;
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

fn wide_os(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RevealTarget {
    Folder(PathBuf),
    Item { folder: PathBuf, name: OsString },
}

/// 将用户要定位的路径拆成 Shell API 所需的「父文件夹 + 子项」。
///
/// 显式拆出父文件夹和子项 PIDL，避免不同 Shell 实现对完整文件 PIDL 的兼容性
/// 差异；失败时只回退到打开父目录，不再调用 `explorer.exe /select`。
fn reveal_target(path: &Path, select: bool) -> RevealTarget {
    if select {
        if let (Some(folder), Some(name)) = (path.parent(), path.file_name()) {
            return RevealTarget::Item {
                folder: folder.to_path_buf(),
                name: name.to_os_string(),
            };
        }
    }
    RevealTarget::Folder(path.to_path_buf())
}

fn parse_pidl(path: &Path) -> Result<*mut c_void, i64> {
    let w = wide_os(path.as_os_str());
    let mut pidl = std::ptr::null_mut();
    let hr = unsafe {
        SHParseDisplayName(
            w.as_ptr(),
            std::ptr::null_mut(),
            &mut pidl,
            0,
            std::ptr::null_mut(),
        )
    };
    if hr < 0 || pidl.is_null() {
        Err(hr as i64)
    } else {
        Ok(pidl)
    }
}

/// 打开一个文件夹 PIDL。
fn open_folder(path: &Path) -> Result<(), i64> {
    let pidl = parse_pidl(path)?;
    let hr = unsafe { SHOpenFolderAndSelectItems(pidl, 0, std::ptr::null(), 0) };
    unsafe { ILFree(pidl) };
    if hr == 0 {
        Ok(())
    } else {
        Err(hr as i64)
    }
}

/// 在父文件夹中选中一个子项 PIDL。
fn select_item(folder: &Path, name: &OsStr) -> Result<(), i64> {
    let full = folder.join(name);
    let full_pidl = parse_pidl(&full)?;
    let folder_pidl = unsafe { ILClone(full_pidl) };
    let child_pidl = unsafe { ILFindLastID(full_pidl) };
    if folder_pidl.is_null() || child_pidl.is_null() {
        unsafe {
            if !folder_pidl.is_null() {
                ILFree(folder_pidl);
            }
            ILFree(full_pidl);
        }
        return Err(-0x7FFF_FFFE);
    }

    let removed = unsafe { ILRemoveLastID(folder_pidl) };
    if removed == 0 {
        unsafe {
            ILFree(folder_pidl);
            ILFree(full_pidl);
        }
        return Err(-0x7FFF_FFFD);
    }

    let child_list = [child_pidl as *const c_void];
    let hr = unsafe { SHOpenFolderAndSelectItems(folder_pidl, 1, child_list.as_ptr(), 0) };
    unsafe {
        ILFree(folder_pidl);
        ILFree(full_pidl);
    }
    if hr == 0 {
        Ok(())
    } else {
        Err(hr as i64)
    }
}

#[repr(C)]
struct ShellExecuteInfoW {
    cb_size: u32,
    f_mask: u32,
    hwnd: isize,
    lp_verb: *const u16,
    lp_file: *const u16,
    lp_parameters: *const u16,
    lp_directory: *const u16,
    n_show: i32,
    h_inst_app: isize,
    lp_id_list: *mut c_void,
    lp_class: *const u16,
    hkey_class: isize,
    dw_hot_key: u32,
    h_icon_or_monitor: isize,
    h_process: isize,
}

const SW_SHOWNORMAL: i32 = 1;
const SEE_MASK_NOASYNC: u32 = 0x0000_0100;

#[link(name = "shell32")]
extern "system" {
    fn ShellExecuteExW(info: *mut ShellExecuteInfoW) -> i32;
}

#[link(name = "kernel32")]
extern "system" {
    fn GetCurrentProcess() -> isize;
    fn CloseHandle(handle: isize) -> i32;
}

#[link(name = "advapi32")]
extern "system" {
    fn OpenProcessToken(process: isize, access: u32, token: *mut isize) -> i32;
    fn GetTokenInformation(
        token: isize,
        class: u32,
        data: *mut c_void,
        data_len: u32,
        return_len: *mut u32,
    ) -> i32;
}

#[repr(C)]
struct TokenElevation {
    token_is_elevated: u32,
}

const TOKEN_QUERY: u32 = 0x0008;
const TOKEN_ELEVATION_CLASS: u32 = 20;

/// 使用 Windows Shell 的标准执行入口打开目录。
///
/// 这里传目录路径本身，不传 `explorer.exe`，因此由 Win10/Win11 当前 Shell
/// 负责复用或创建资源管理器窗口，不会让应用自行拉起 Explorer 子进程。
fn shell_execute_open(path: &Path) -> Result<(), i64> {
    let verb = wide_os(OsStr::new("open"));
    let file = wide_os(path.as_os_str());
    let mut info = ShellExecuteInfoW {
        cb_size: std::mem::size_of::<ShellExecuteInfoW>() as u32,
        f_mask: SEE_MASK_NOASYNC,
        hwnd: 0,
        lp_verb: verb.as_ptr(),
        lp_file: file.as_ptr(),
        lp_parameters: std::ptr::null(),
        lp_directory: std::ptr::null(),
        n_show: SW_SHOWNORMAL,
        h_inst_app: 0,
        lp_id_list: std::ptr::null_mut(),
        lp_class: std::ptr::null(),
        hkey_class: 0,
        dw_hot_key: 0,
        h_icon_or_monitor: 0,
        h_process: 0,
    };
    if unsafe { ShellExecuteExW(&mut info) } != 0 {
        Ok(())
    } else {
        let code = std::io::Error::last_os_error().raw_os_error().unwrap_or(-1);
        Err(code as i64)
    }
}

fn quote_windows_arg(value: &OsStr) -> String {
    let mut out = String::from("\"");
    let mut slashes = 0usize;
    for ch in value.to_string_lossy().chars() {
        if ch == '\\' {
            slashes += 1;
            continue;
        }
        if ch == '"' {
            out.push_str(&"\\".repeat(slashes * 2 + 1));
            out.push('"');
        } else {
            out.push_str(&"\\".repeat(slashes));
            out.push(ch);
        }
        slashes = 0;
    }
    out.push_str(&"\\".repeat(slashes * 2));
    out.push('"');
    out
}

fn is_access_denied(code: i64) -> bool {
    code == 5 || code == -2_147_024_891
}

fn should_request_elevation(code: i64) -> bool {
    is_access_denied(code) || code == -2
}

fn is_process_elevated() -> bool {
    let mut token = 0isize;
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return false;
    }
    let mut elevation = TokenElevation {
        token_is_elevated: 0,
    };
    let mut returned = 0u32;
    let ok = unsafe {
        GetTokenInformation(
            token,
            TOKEN_ELEVATION_CLASS,
            (&mut elevation as *mut TokenElevation).cast(),
            std::mem::size_of::<TokenElevation>() as u32,
            &mut returned,
        )
    } != 0;
    unsafe { CloseHandle(token) };
    ok && elevation.token_is_elevated != 0
}

fn preserve_access_denied<F>(first: Result<(), i64>, fallback: F) -> Result<(), i64>
where
    F: FnOnce() -> Result<(), i64>,
{
    match first {
        Ok(()) => Ok(()),
        Err(code) if should_request_elevation(code) => Err(code),
        Err(_) => fallback(),
    }
}

fn relaunch_elevated_helper(path: &str, select: bool) -> Result<(), i64> {
    let exe = std::env::current_exe().map_err(|e| e.raw_os_error().unwrap_or(-1) as i64)?;
    let verb = wide_os(OsStr::new("runas"));
    let file = wide_os(exe.as_os_str());
    let params = format!(
        "--reveal-helper {} {}",
        quote_windows_arg(OsStr::new(path)),
        if select { "1" } else { "0" }
    );
    let parameters = wide_os(OsStr::new(&params));
    let mut info = ShellExecuteInfoW {
        cb_size: std::mem::size_of::<ShellExecuteInfoW>() as u32,
        f_mask: SEE_MASK_NOASYNC,
        hwnd: 0,
        lp_verb: verb.as_ptr(),
        lp_file: file.as_ptr(),
        lp_parameters: parameters.as_ptr(),
        lp_directory: std::ptr::null(),
        n_show: SW_SHOWNORMAL,
        h_inst_app: 0,
        lp_id_list: std::ptr::null_mut(),
        lp_class: std::ptr::null(),
        hkey_class: 0,
        dw_hot_key: 0,
        h_icon_or_monitor: 0,
        h_process: 0,
    };
    if unsafe { ShellExecuteExW(&mut info) } != 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error().raw_os_error().unwrap_or(-1) as i64)
    }
}

fn fallback_open_folder(path: &Path) -> Result<(), i64> {
    shell_execute_open(path)
}

fn open_folder_with_fallback(folder: &Path, first: Result<(), i64>) -> Result<(), i64> {
    preserve_access_denied(first, || fallback_open_folder(folder))
}

fn reveal_item_with_fallback(folder: &Path, name: &OsStr) -> Result<(), i64> {
    preserve_access_denied(select_item(folder, name), || {
        open_folder_with_fallback(folder, open_folder(folder))
    })
}

/// 打开资源管理器并定位 path：文件就选中它，目录就打开它。
///
/// 成功返回 Ok(())，失败返回最后一次外壳调用的 HRESULT（十六进制方便对照文档）。
/// 把错误码带出去是刻意的：这类调用失败时系统只给一个数字，没有它根本没法排查。
///
/// 为什么必须另起线程：SHOpenFolderAndSelectItems 是 COM 调用，要求调用线程是
/// 单线程套间（STA）。而 egui 的主线程由 winit 建出来，套间模型不受我们控制，
/// 所以起一个专门的新线程，先 CoInitializeEx(APARTMENTTHREADED) 再调用，就一定在 STA 里。
pub fn reveal(path: &str, select: bool) -> Result<(), i64> {
    if !is_process_elevated() {
        return relaunch_elevated_helper(path, select);
    }
    match reveal_once(path, select) {
        Err(code) if should_request_elevation(code) => relaunch_elevated_helper(path, select),
        result => result,
    }
}

pub fn reveal_helper(path: &str, select: bool) -> Result<(), i64> {
    reveal_once(path, select)
}

fn reveal_once(path: &str, select: bool) -> Result<(), i64> {
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

    let path_obj = Path::new(path);
    let target = reveal_target(path_obj, select);
    let result = match &target {
        RevealTarget::Item { folder, name } => {
            let result = reveal_item_with_fallback(folder, name);
            log.push_str(&format!(
                "select item [{}\\{}]: {:?}\n",
                folder.display(),
                name.to_string_lossy(),
                result
            ));
            result
        }
        RevealTarget::Folder(folder) => {
            let result = open_folder_with_fallback(folder, open_folder(folder));
            log.push_str(&format!("open folder [{}]: {:?}\n", folder.display(), result));
            result
        }
    };

    if hr_init < 0 {
        log.push_str(&format!("CoInitializeEx failed: 0x{hr_init:08X}\n"));
    }
    flush_log(&log);
    result
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

#[cfg(test)]
mod tests {
    use super::{is_access_denied, should_request_elevation, quote_windows_arg, reveal_target, RevealTarget, preserve_access_denied};
    use std::path::Path;

    #[test]
    fn denial_is_not_replaced_by_a_later_failure_or_false_fallback_success() {
        let mut called = false;
        let result = preserve_access_denied(Err(-2_147_024_891), || {
            called = true;
            Ok(())
        });
        assert_eq!(result, Err(-2_147_024_891));
        assert!(!called);
    }

    #[test]
    fn unrelated_failure_can_use_a_fallback() {
        assert_eq!(preserve_access_denied(Err(2), || Ok(())), Ok(()));
        assert_eq!(preserve_access_denied(Ok(()), || Err(5)), Ok(()));
    }

    #[test]
    fn access_denied_codes_trigger_elevation_policy() {
        assert!(is_access_denied(5));
        assert!(is_access_denied(-2_147_024_891));
        assert!(!is_access_denied(2));
    }

    #[test]
    fn a_shell_timeout_also_requests_elevation() {
        assert!(should_request_elevation(-2));
        assert!(should_request_elevation(5));
        assert!(!should_request_elevation(2));
    }

    #[test]
    fn elevated_helper_quotes_windows_paths() {
        assert_eq!(
            quote_windows_arg(std::ffi::OsStr::new(r"C:\Program Files\OTA\file.zip")),
            r#""C:\Program Files\OTA\file.zip""#
        );
        assert_eq!(
            quote_windows_arg(std::ffi::OsStr::new(r"C:\Program Files\OTA\")),
            r#""C:\Program Files\OTA\\""#
        );
    }

    #[test]
    fn file_selection_uses_parent_folder_and_child_name() {
        let target = reveal_target(Path::new(r"C:\Users\GuLi\Downloads\package.zip"), true);
        assert_eq!(
            target,
            RevealTarget::Item {
                folder: Path::new(r"C:\Users\GuLi\Downloads").to_path_buf(),
                name: "package.zip".into(),
            }
        );
    }

    #[test]
    fn directory_open_uses_the_directory_pidls_directly() {
        let target = reveal_target(Path::new(r"C:\Users\GuLi\Downloads"), false);
        assert_eq!(
            target,
            RevealTarget::Folder(Path::new(r"C:\Users\GuLi\Downloads").to_path_buf())
        );
    }
}
