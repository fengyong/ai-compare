//! `aic` —— ai-compare CLI（进程内链接引擎库；docs/03 §2.1 关键约束 2）。
//!
//! 用法：
//! ```text
//! aic diff <left> <right> [-U N] [--color auto|always|never]   文本 unified diff；
//!                                                              两侧为目录时递归输出（diff -r 风格）
//! aic quick <left> <right> [--mode size|crc|binary|rules]      快速比较（文件或目录）
//! ```
//!
//! 退出码（docs/04 P0/P3）：0 相同 / 1 不同 / 2 错误。
//! `--mode rules`：基于规则的目录比较（P2 以二进制判等占位，规则接口 P10 接入
//! —— docs/04 P2 依赖注记）。

use std::io::{IsTerminal, Write as _};
use std::process::ExitCode;

use aic::{Palette, ANSI, EXIT_DIFF, EXIT_ERROR, EXIT_SAME, PLAIN};
use ai_core::{LineTable, TextDiffOptions};
use ai_protocol::CompareCriteria;
use ai_vfs::{LocalFs, VfsPath, VfsProvider as _};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("diff") => cmd_diff(&args[1..]),
        Some("quick") => cmd_quick(&args[1..]),
        Some("--help") | Some("-h") | Some("help") => {
            print_usage();
            EXIT_SAME.into()
        }
        Some("--version") | Some("-V") => {
            println!("aic {}", env!("CARGO_PKG_VERSION"));
            EXIT_SAME.into()
        }
        _ => {
            print_usage();
            EXIT_ERROR.into()
        }
    }
}

fn print_usage() {
    eprintln!(
        "aic —— ai-compare 命令行\n\
         \n\
         用法:\n  \
         aic diff <left> <right> [-U <n>] [--color auto|always|never]   unified diff 控制台输出\n  \
         aic quick <left> <right> [--mode size|crc|binary]              快速比较（文件或目录）\n\
         \n\
         退出码: 0 相同 / 1 不同 / 2 错误\n\
         目录 quick: size=仅大小; crc=分块哈希; binary=与 crc 同级（目录判等走扫描内容比较）"
    );
}

// ---------------------------------------------------------------------------
// aic diff
// ---------------------------------------------------------------------------

fn cmd_diff(args: &[String]) -> ExitCode {
    let mut positional: Vec<&String> = Vec::new();
    let mut context: u32 = 3;
    let mut color = "auto".to_string();
    let mut i = 0usize;
    while i < args.len() {
        let a = args[i].as_str();
        match a {
            "-U" | "--unified" => {
                i += 1;
                match args.get(i).and_then(|v| v.parse::<u32>().ok()) {
                    Some(n) => context = n,
                    None => return arg_error("-U 需要数字参数"),
                }
            }
            _ if a.starts_with("-U") && a.len() > 2 => match a[2..].parse::<u32>() {
                Ok(n) => context = n,
                Err(_) => return arg_error(&format!("非法 -U 值: {a}")),
            },
            "--color" => {
                i += 1;
                match args.get(i).map(String::as_str) {
                    Some(c @ ("auto" | "always" | "never")) => color = c.to_string(),
                    _ => return arg_error("--color 仅支持 auto|always|never"),
                }
            }
            _ if a.starts_with('-') => return arg_error(&format!("未知参数: {a}")),
            _ => positional.push(&args[i]),
        }
        i += 1;
    }
    if positional.len() != 2 {
        return arg_error("diff 需要恰好两个路径参数: <left> <right>");
    }

    let palette: &Palette = match color.as_str() {
        "always" => &ANSI,
        "never" => &PLAIN,
        _ if std::io::stdout().is_terminal() => &ANSI,
        _ => &PLAIN,
    };

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio 运行时");
    let result = rt.block_on(run_diff(positional[0], positional[1], context, palette));
    match result {
        Ok(()) => EXIT_SAME.into(),
        Err(DiffOutcome::Different) => EXIT_DIFF.into(),
        Err(DiffOutcome::Error(msg)) => {
            eprintln!("aic diff: {msg}");
            EXIT_ERROR.into()
        }
    }
}

async fn run_diff(
    left_s: &str,
    right_s: &str,
    context: u32,
    palette: &Palette,
) -> Result<(), DiffOutcome> {
    let vfs = LocalFs::new();
    let left_path = VfsPath::parse(left_s).map_err(|e| DiffOutcome::Error(e.to_string()))?;
    let right_path = VfsPath::parse(right_s).map_err(|e| DiffOutcome::Error(e.to_string()))?;

    // 目录递归输出（P2：aic diff 对接目录引擎，diff -r 风格摘要）
    {
        let ls = vfs.stat(&left_path).await.map_err(|e| DiffOutcome::Error(e.to_string()))?;
        let rs = vfs.stat(&right_path).await.map_err(|e| DiffOutcome::Error(e.to_string()))?;
        if ls.kind == ai_vfs::EntryKind::Dir && rs.kind == ai_vfs::EntryKind::Dir {
            return run_dir_diff(&vfs, &left_path, &right_path, left_s, right_s).await;
        }
        if ls.kind == ai_vfs::EntryKind::Dir || rs.kind == ai_vfs::EntryKind::Dir {
            return Err(DiffOutcome::Error("目录只能与目录比较".into()));
        }
    }

    let left_text = vfs
        .read_file_text(&left_path)
        .await
        .map_err(|e| DiffOutcome::Error(e.to_string()))?;
    let right_text = vfs
        .read_file_text(&right_path)
        .await
        .map_err(|e| DiffOutcome::Error(e.to_string()))?;

    let lt = LineTable::from_text(&left_text);
    let rt = LineTable::from_text(&right_text);
    let out = ai_core::diff_text(&lt, &rt, &TextDiffOptions::default())
        .map_err(|e| DiffOutcome::Error(e.to_string()))?;

    if out.sections.iter().all(|s| s.kind == ai_core::SectionKind::Equal) {
        return Ok(()); // 相同：无输出，退出码 0
    }
    let text = aic::render_unified(left_s, right_s, &out, &lt, &rt, context, palette);
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    let _ = lock.write_all(text.as_bytes());
    let _ = lock.flush();
    Err(DiffOutcome::Different)
}

/// 目录递归 diff：ai-dir 全量扫描 -> `diff -rq` 风格摘要输出（对接 P2 引擎能力）。
/// 判据：内容比较 Always + 容差 0（与 `diff -r` 的内容语义一致，不按 mtime 判同）。
async fn run_dir_diff(
    vfs: &LocalFs,
    left: &VfsPath,
    right: &VfsPath,
    left_s: &str,
    right_s: &str,
) -> Result<(), DiffOutcome> {
    let result = ai_dir::scan(
        vfs,
        left,
        right,
        &ai_dir::DirScanOptions {
            depth: None,
            criteria: CompareCriteria {
                mtime_tolerance_sec: 0,
                content_compare: ai_protocol::ContentCompareMode::Always,
                ..Default::default()
            },
        },
        &ai_dir::ByNameAligner,
        &ai_dir::CancelFlag::new(),
    )
    .await
    .map_err(|e| DiffOutcome::Error(e.to_string()))?;
    let text = aic::render_dir_diff(left_s, right_s, &result.arena);
    if text.is_empty() {
        return Ok(());
    }
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    let _ = lock.write_all(text.as_bytes());
    let _ = lock.flush();
    Err(DiffOutcome::Different)
}

enum DiffOutcome {
    Different,
    Error(String),
}

// ---------------------------------------------------------------------------
// aic quick
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QuickMode {
    Size,
    Crc,
    Binary,
    /// 基于规则的比较（P2 以二进制判等占位；规则接口 P10 接入）
    Rules,
}

impl QuickMode {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "size" => Some(QuickMode::Size),
            "crc" => Some(QuickMode::Crc),
            "binary" => Some(QuickMode::Binary),
            "rules" => Some(QuickMode::Rules),
            _ => None,
        }
    }
    fn as_str(&self) -> &'static str {
        match self {
            QuickMode::Size => "size",
            QuickMode::Crc => "crc",
            QuickMode::Binary => "binary",
            QuickMode::Rules => "rules",
        }
    }
}

fn cmd_quick(args: &[String]) -> ExitCode {
    let mut positional: Vec<&String> = Vec::new();
    let mut mode = QuickMode::Crc;
    let mut i = 0usize;
    while i < args.len() {
        let a = args[i].as_str();
        match a {
            "--mode" | "-m" => {
                i += 1;
                match args.get(i).map(String::as_str).and_then(QuickMode::parse) {
                    Some(m) => mode = m,
                    None => return arg_error("--mode 仅支持 size|crc|binary|rules"),
                }
            }
            _ if a.starts_with('-') => return arg_error(&format!("未知参数: {a}")),
            _ => positional.push(&args[i]),
        }
        i += 1;
    }
    if positional.len() != 2 {
        return arg_error("quick 需要恰好两个路径参数: <left> <right>");
    }

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio 运行时");
    match rt.block_on(run_quick(positional[0], positional[1], mode)) {
        Ok(same) => {
            if same {
                EXIT_SAME.into()
            } else {
                EXIT_DIFF.into()
            }
        }
        Err(msg) => {
            eprintln!("aic quick: {msg}");
            EXIT_ERROR.into()
        }
    }
}

async fn run_quick(left_s: &str, right_s: &str, mode: QuickMode) -> Result<bool, String> {
    let vfs = LocalFs::new();
    let left = VfsPath::parse(left_s).map_err(|e| e.to_string())?;
    let right = VfsPath::parse(right_s).map_err(|e| e.to_string())?;
    let ls = vfs.stat(&left).await.map_err(|e| e.to_string())?;
    let rs = vfs.stat(&right).await.map_err(|e| e.to_string())?;

    if ls.kind == ai_vfs::EntryKind::Dir || rs.kind == ai_vfs::EntryKind::Dir {
        // 目录 quick：走 ai-dir 扫描（全递归）
        if ls.kind != rs.kind {
            return Err("目录只能与目录比较".to_string());
        }
        // size 模式 = 仅大小（mtime 容差拉满 + 不做内容比较）；
        // crc/binary = 内容比较（目录判等内部分块哈希，同长度内容快于逐字节场景）
        let criteria = match mode {
            QuickMode::Size => CompareCriteria {
                mtime_tolerance_sec: u32::MAX,
                content_compare: ai_protocol::ContentCompareMode::Never,
                ..Default::default()
            },
            // crc/binary/rules：目录判等走内容比较（rules 在 P10 前以二进制占位）
            QuickMode::Crc | QuickMode::Binary | QuickMode::Rules => CompareCriteria {
                mtime_tolerance_sec: 0,
                content_compare: ai_protocol::ContentCompareMode::Always,
                ..Default::default()
            },
        };
        let result = ai_dir::scan(
            &vfs,
            &left,
            &right,
            &ai_dir::DirScanOptions { depth: None, criteria },
            &ai_dir::ByNameAligner,
            &ai_dir::CancelFlag::new(),
        )
        .await
        .map_err(|e| e.to_string())?;
        let same = result.stats.content_diff == 0
            && result.stats.quick_diff == 0
            && result.stats.left_only == 0
            && result.stats.right_only == 0;
        println!(
            "{}: {left_s} {right_s} (mode={}, rows={}, contentDiff={}, orphans=L{}/R{})",
            if same { "same" } else { "differ" },
            mode.as_str(),
            result.stats.total,
            result.stats.content_diff,
            result.stats.left_only,
            result.stats.right_only,
        );
        Ok(same)
    } else {
        // 文件 quick
        let same = match mode {
            QuickMode::Size => ls.size == rs.size,
            QuickMode::Crc => {
                ls.size == rs.size
                    && ai_dir::content_same(&vfs, &left, &right, ai_dir::ContentHashMode::Crc)
                        .await
                        .map_err(|e| e.to_string())?
            }
            // rules（P10 前以二进制占位，docs/04 P2 依赖注记）
            QuickMode::Binary | QuickMode::Rules => {
                ls.size == rs.size
                    && ai_dir::content_same(&vfs, &left, &right, ai_dir::ContentHashMode::Binary)
                        .await
                        .map_err(|e| e.to_string())?
            }
        };
        println!(
            "{}: {left_s} {right_s} (mode={}, size={}B)",
            if same { "same" } else { "differ" },
            mode.as_str(),
            ls.size
        );
        Ok(same)
    }
}

fn arg_error(msg: &str) -> ExitCode {
    eprintln!("aic: {msg}");
    print_usage();
    EXIT_ERROR.into()
}
