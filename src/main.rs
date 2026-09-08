use colored::Colorize;
use dialoguer::Select;
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const CONFIG_FILE: &str = "config.yaml";

/// 首次运行时生成的默认配置文件内容（带注释说明）
const DEFAULT_CONFIG_TEMPLATE: &str = r#"# 视频下载工具配置文件
# 修改后保存，下次启动时自动生效
# 配置文件应与程序放在同一目录下

# 1. 视频下载默认目录（默认：程序所在目录）
#    支持相对路径（相对于程序所在目录），例如: ./downloads
download_dir: "."

# 2. yt-dlp 程序位置（默认：程序所在目录下的 yt-dlp / yt-dlp.exe）
#    可填绝对路径，也可直接填命令名（如: yt-dlp，将从系统 PATH 中查找）
yt_dlp_path: "yt-dlp"
"#;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct Config {
    download_dir: String,
    yt_dlp_path: String,
    start_time: String,
    end_time: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            download_dir: ".".to_string(),
            yt_dlp_path: "yt-dlp".to_string(),
            start_time: String::new(),
            end_time: String::new(),
        }
    }
}

/// yt-dlp 的位置：位于某个文件，或仅是一个可从 PATH 中解析的命令名
enum YtDlpLoc {
    File(PathBuf),
    InPath(String),
}

fn main() -> ExitCode {
    // 将程序所在目录作为工作目录基准
    let exe_dir = program_dir();
    let config_path = exe_dir.join(CONFIG_FILE);

    // 1. 无配置文件则先生成，再读取
    let config = match ensure_config(&config_path) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("{} 读取配置文件失败: {}", "错误:".red().bold(), e);
            return ExitCode::FAILURE;
        }
    };

    let download_dir = resolve_against(&exe_dir, Path::new(&config.download_dir));

    // 2. 欢迎消息：展示默认下载目录与 yt-dlp 版本
    println!("==================================================");
    println!("             视频下载 yt-dlp 工具");
    println!("==================================================");
    println!("配置文件:     {}", config_path.display().to_string().bright_black());
    println!("下载默认目录: {}", download_dir.display().to_string().cyan());

    let yt = match resolve_yt_dlp(&exe_dir, Path::new(&config.yt_dlp_path)) {
        Some(loc) => loc,
        None => {
            println!(
                "{} yt-dlp 不存在，请检查配置文件中的 yt_dlp_path，\
                 或下载 yt-dlp 后与程序放在同一目录再重试。",
                "yt-dlp 不存在".red().bold()
            );
            return ExitCode::FAILURE;
        }
    };

    match yt_version(&yt) {
        Some(v) => println!("yt-dlp 版本:   {}", v.green()),
        None => {
            println!("{} 无法获取 yt-dlp 版本信息。", "警告:".yellow());
            return ExitCode::FAILURE;
        }
    }
    println!();

    // 3. 交互输入：下载地址与文件名
    let url = ask_required("请输入视频下载地址: ");
    let name = ask_required("请输入下载后的文件名: ");

    // 4. 下载前用下拉选项选择下载范围
    let section = match ask_range(&config) {
        Ok(s) => s,
        Err(e) => {
            println!("{} {}", "已取消:".red().bold(), e);
            return ExitCode::SUCCESS;
        }
    };

    // 5. 准备输出目录并执行下载
    if let Err(e) = fs::create_dir_all(&download_dir) {
        println!(
            "{} 无法创建下载目录 {}: {}",
            "错误:".red().bold(),
            download_dir.display(),
            e
        );
        return ExitCode::FAILURE;
    }
    let output_template = download_dir.join(format!("{}.%(ext)s", name));

    println!("\n视频将保存到: {}\n", output_template.display().to_string().cyan());

    let mut cmd = match &yt {
        YtDlpLoc::File(p) => Command::new(p),
        YtDlpLoc::InPath(n) => Command::new(n),
    };
    cmd.arg(&url);
    if let Some(sec) = &section {
        cmd.args(["--download-sections", sec]);
    }
    cmd.arg("-o").arg(&output_template);
    cmd.args([
        "--downloader",
        "ffmpeg",
        "--downloader-args",
        "ffmpeg:-map 0",
    ]);

    println!("正在开始下载...");
    match cmd.status() {
        Ok(st) if st.success() => {
            println!("\n{}", "下载完成！".green().bold());
        }
        Ok(_) => {
            println!("\n{}", "下载过程中出现错误，请检查链接、文件名或时间格式。".red().bold());
            return ExitCode::FAILURE;
        }
        Err(e) => {
            println!("\n{} 无法启动 yt-dlp: {}", "错误:".red().bold(), e);
            return ExitCode::FAILURE;
        }
    }

    ExitCode::SUCCESS
}

/// 程序所在目录
fn program_dir() -> PathBuf {
    env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .or_else(|| env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// 配置不存在时生成模板文件，然后读取并解析
fn ensure_config(path: &Path) -> Result<Config, String> {
    if !path.exists() {
        fs::write(path, DEFAULT_CONFIG_TEMPLATE).map_err(|e| format!("无法生成配置文件 {}: {}", path.display(), e))?;
        println!("{} 未找到配置文件，已在 {} 生成默认配置。", "提示:".yellow(), path.display().to_string().cyan());
        println!("如需调整下载目录 / yt-dlp 位置 / 下载范围，请编辑该文件后重新运行。\n");
    }
    let text = fs::read_to_string(path).map_err(|e| format!("无法读取 {}: {}", path.display(), e))?;
    serde_yaml::from_str(&text).map_err(|e| format!("{} 内容解析失败: {}", path.display(), e))
}

/// 相对路径基于 base（程序所在目录）解析
fn resolve_against(base: &Path, p: &Path) -> PathBuf {
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        base.join(p)
    }
}

/// 依据配置定位 yt-dlp：
/// 1) 配置路径（相对程序目录解析）直接存在
/// 2) Windows 下自动补 .exe 后缀
/// 3) 若配置为纯命令名（不含路径分隔符），从系统 PATH 中探测
fn resolve_yt_dlp(exe_dir: &Path, configured: &Path) -> Option<YtDlpLoc> {
    let abs = resolve_against(exe_dir, configured);
    if abs.exists() {
        return Some(YtDlpLoc::File(abs));
    }
    #[cfg(windows)]
    {
        let mut exe = abs.clone();
        if exe.extension().is_none() {
            exe.set_extension("exe");
            if exe.exists() {
                return Some(YtDlpLoc::File(exe));
            }
        }
    }
    // 纯命令名（不含路径分隔符）时尝试从 PATH 查找
    let name = configured.to_string_lossy();
    let has_sep = name.contains('/') || (cfg!(windows) && name.contains('\\'));
    if !has_sep && !name.is_empty() {
        if let Ok(out) = Command::new(&*name).arg("--version").output() {
            if out.status.success() {
                return Some(YtDlpLoc::InPath(name.to_string()));
            }
        }
    }
    None
}

/// 获取 yt-dlp 版本号
fn yt_version(yt: &YtDlpLoc) -> Option<String> {
    let out = match yt {
        YtDlpLoc::File(p) => Command::new(p).arg("--version").output(),
        YtDlpLoc::InPath(n) => Command::new(n).arg("--version").output(),
    }
    .ok()?;
    if !out.status.success() {
        return None;
    }
    let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}

/// 下载范围的可选项
const RANGE_ITEMS: [&str; 3] = ["全部下载", "仅下载前 3 分钟", "自定义片段"];

/// 下载前用下拉选项选择下载范围，返回 --download-sections 参数（全部下载时为 None）
fn ask_range(config: &Config) -> Result<Option<String>, String> {
    let choice = if io::stdin().is_terminal() {
        // 交互式终端：使用方向键下拉选择
        Select::new()
            .with_prompt("请选择下载范围（↑ ↓ 移动，回车确认）")
            .items(&RANGE_ITEMS)
            .default(0)
            .interact_opt()
            .map_err(|e| format!("读取选择失败: {}", e))?
            .ok_or_else(|| "用户取消了选择".to_string())?
    } else {
        // 非交互环境（管道 / 重定向）退化为数字输入，便于脚本调用
        println!("\n请选择下载范围:");
        for (i, item) in RANGE_ITEMS.iter().enumerate() {
            println!("  [{}] {}", i + 1, item);
        }
        loop {
            match read_line("请输入选择 [1/2/3]（回车默认 1 全部下载）: ").as_str() {
                "" | "1" => break 0usize,
                "2" => break 1,
                "3" => break 2,
                _ => println!("{} 无效选择，请输入 1、2 或 3。", "提示:".yellow()),
            }
        }
    };

    match choice {
        1 => {
            println!("下载范围: {}", "前 3 分钟".cyan());
            Ok(Some("*00:00:00-00:03:00".to_string()))
        }
        2 => {
            let start_default = if config.start_time.trim().is_empty() {
                "00:00:00".to_string()
            } else {
                config.start_time.trim().to_string()
            };
            let end_default = if config.end_time.trim().is_empty() {
                "inf".to_string()
            } else {
                config.end_time.trim().to_string()
            };
            let start = ask_with_default(&format!("请输入开始时间（回车默认 {}）: ", start_default), &start_default);
            let end = ask_with_default(
                &format!("请输入结束时间（inf 表示视频结束，回车默认 {}）: ", end_default),
                &end_default,
            );
            println!("下载范围: {} - {}", start.cyan(), end.cyan());
            Ok(Some(format!("*{}-{}", start, end)))
        }
        _ => {
            println!("下载范围: {}", "全部".cyan());
            Ok(None)
        }
    }
}

/// 读取一行输入
fn read_line(prompt: &str) -> String {
    print!("{}", prompt);
    let _ = io::stdout().flush();
    let mut s = String::new();
    match io::stdin().read_line(&mut s) {
        Ok(_) => s.trim().to_string(),
        Err(_) => String::new(),
    }
}

/// 必填输入，为空时循环提示
fn ask_required(prompt: &str) -> String {
    loop {
        let s = read_line(prompt);
        if !s.is_empty() {
            return s;
        }
        println!("{} 输入不能为空，请重新输入。", "提示:".yellow());
    }
}

/// 可选输入，为空时返回默认值
fn ask_with_default(prompt: &str, default: &str) -> String {
    let s = read_line(prompt);
    if s.is_empty() {
        default.to_string()
    } else {
        s
    }
}
