//! 界面层：状态 + 视图
pub mod format;
/// 资源管理器定位（Windows 外壳调用，单独一份好写清踩过的坑）
#[cfg(windows)]
pub mod shell;
pub mod state;
pub mod views;

/// 非 Windows 平台没有资源管理器，定位一律失败，由上层退回剪贴板兜底
#[cfg(not(windows))]
pub mod shell {
    pub fn reveal(_path: &str, _select: bool) -> Result<(), i64> {
        Err(-1)
    }
}

#[allow(unused_imports)]
pub use state::{App, Model, Tone};
