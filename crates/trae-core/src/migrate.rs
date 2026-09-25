// 旧 TraeMate 数据迁移:把 %APPDATA%\com.traecheck.app 下的数据只读拷贝到新目录。
// - 主存储 trae-check-data.json → <trae_dir>/trae-check-data.json(其中 launch_at_login 强制
//   复位为 false:旧自启项指向旧应用,保留会双开抢 TRAE 实例)
// - data/*.json → <trae_dir>/data/
// - trae_exe_path.txt → <config_dir>(如宿主提供;TRAE 安装路径配置)
// 幂等:目标文件已存在且非空 → 跳过,重复调用安全。凭证为 DPAPI 密文,绑定 Windows 用户,
// 同用户拷贝后可直接解密;跨用户解密失败由签到路径报错兜底,本函数不做加解密。

use serde::Serialize;
use std::path::Path;

/// 旧 TraeMate 的 Tauri app_data_dir 目录名(identifier)
pub const LEGACY_DIR_NAME: &str = "com.traecheck.app";
/// 旧 TraeMate 进程名(productName),迁移报告据此提示双开竞争
pub const LEGACY_PROCESS_NAME: &str = "TraeMate";

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MigrationReport {
    /// 是否探测到旧数据目录
    pub detected: bool,
    /// 迁移的账号数(主存储内 accounts 长度)
    pub accounts_imported: usize,
    /// 迁移的日志条数
    pub logs_imported: usize,
    /// 成功迁移的文件相对路径(相对 trae_dir)
    pub files: Vec<String>,
    /// 跳过的文件与原因
    pub skipped: Vec<String>,
    /// TRAE exe 路径配置是否迁移成功
    pub exe_path_migrated: bool,
    /// 检测到旧 TraeMate 进程仍在运行(提示卸载/退出旧版,避免双开竞争调度)
    pub legacy_process_running: bool,
}

/// 旧数据目录(%APPDATA%\com.traecheck.app)。非 Windows / 未安装返回 None。
fn legacy_dir() -> Option<std::path::PathBuf> {
    let appdata = std::env::var("APPDATA").ok()?;
    let dir = std::path::PathBuf::from(appdata).join(LEGACY_DIR_NAME);
    dir.is_dir().then_some(dir)
}

/// 检测旧 TraeMate 进程是否在运行(tasklist 按映像名过滤)。非 Windows 恒 false。
fn legacy_process_running() -> bool {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        let Ok(out) = std::process::Command::new("tasklist")
            .args(["/FI", &format!("IMAGENAME eq {LEGACY_PROCESS_NAME}.exe"), "/NH", "/FO", "CSV"])
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW,避免弹黑窗
            .output()
        else {
            return false;
        };
        let text = String::from_utf8_lossy(&out.stdout);
        text.contains(LEGACY_PROCESS_NAME)
    }
    #[cfg(not(target_os = "windows"))]
    {
        false
    }
}

/// 拷贝单个文件,目标已存在且非空时跳过(幂等)。返回 Ok(Some(跳过原因))。
fn copy_if_absent(src: &Path, dst: &Path) -> Result<(), String> {
    if dst.exists() {
        match std::fs::metadata(dst) {
            Ok(m) if m.len() > 0 => {
                return Err("目标已存在,跳过".into());
            }
            _ => {}
        }
    }
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建目录失败: {e}"))?;
    }
    std::fs::copy(src, dst).map_err(|e| format!("拷贝失败: {e}"))?;
    Ok(())
}

/// 主存储迁移:拷贝 + 把 launch_at_login 复位为 false(旧自启项指向旧应用)。
fn migrate_store(src: &Path, dst: &Path, report: &mut MigrationReport) {
    match copy_if_absent(src, dst) {
        Ok(()) => {
            // 读回统计账号/日志数并复位 launch_at_login
            let raw = std::fs::read_to_string(dst).unwrap_or_default();
            if let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&raw) {
                report.accounts_imported = v
                    .get("accounts")
                    .and_then(|a| a.as_array())
                    .map(|a| a.len())
                    .unwrap_or(0);
                report.logs_imported = v
                    .get("logs")
                    .and_then(|l| l.as_array())
                    .map(|l| l.len())
                    .unwrap_or(0);
                let mut changed = false;
                if let Some(settings) = v.get_mut("settings") {
                    if settings.get("launchAtLogin").and_then(|b| b.as_bool()) == Some(true) {
                        settings["launchAtLogin"] = serde_json::Value::Bool(false);
                        changed = true;
                    }
                }
                if changed {
                    if let Ok(pretty) = serde_json::to_string_pretty(&v) {
                        let _ = std::fs::write(dst, pretty);
                    }
                }
            }
            report.files.push("trae-check-data.json".into());
        }
        Err(reason) => report.skipped.push(format!("trae-check-data.json: {reason}")),
    }
}

/// 执行迁移。`trae_dir` 为新数据根(~/.wb-switch/trae/);`config_dir` 为宿主配置目录
/// (提供时迁移 TRAE exe 路径配置)。可重复调用(幂等),setup 与手动命令共用。
pub fn migrate_from_legacy(trae_dir: &Path, config_dir: Option<&Path>) -> MigrationReport {
    let mut report = MigrationReport::default();

    let Some(src_dir) = legacy_dir() else {
        return report; // 未检测到旧目录,detected=false,静默结束
    };
    report.detected = true;
    report.legacy_process_running = legacy_process_running();

    // 1. 主存储
    migrate_store(&src_dir.join("trae-check-data.json"), &trae_dir.join("trae-check-data.json"), &mut report);

    // 2. data/*.json
    let src_data = src_dir.join("data");
    if src_data.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&src_data) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if !name.ends_with(".json") {
                    continue;
                }
                let rel = format!("data/{name}");
                match copy_if_absent(&entry.path(), &trae_dir.join("data").join(&name)) {
                    Ok(()) => report.files.push(rel),
                    Err(reason) => report.skipped.push(format!("{rel}: {reason}")),
                }
            }
        }
    }

    // 3. TRAE exe 路径配置(宿主 config_dir)
    if let Some(cfg) = config_dir {
        let src_cfg = src_dir.join("trae_exe_path.txt");
        if src_cfg.exists() {
            match copy_if_absent(&src_cfg, &cfg.join("trae_exe_path.txt")) {
                Ok(()) => report.exe_path_migrated = true,
                Err(reason) => {
                    report.skipped.push(format!("trae_exe_path.txt: {reason}"));
                }
            }
        }
    }

    report
}
