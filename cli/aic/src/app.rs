//! `aic` 命令实现（CLI 全集，docs/04 §P3 / docs/01 §4.8）：
//!
//! - [`run_cli`]：aic 主入口（diff / quick / help / version）；
//! - `aicc`（bin/aicc.rs）复用本模块作为进程内回退路径。
//!
//! 命令面：
//! ```text
//! aic diff <left> <right> [--algo standard|myers|patience|histogram]
//!          [-w|--ignore-whitespace] [--ignore-whitespace-amount]
//!          [--ignore-case] [--compare-eol] [-U <n>] [--color auto|always|never]
//!          （两侧为目录时递归输出，diff -r 风格）
//! aic quick <left> <right> [--mode size|crc|binary|rules] [--ignore-whitespace] [--ignore-case]
//! ```
//!
//! 退出码（FR-CLI-004 兼容分层，见 lib.rs 常量文档）：
//! 0 相同/成功、1 不同(diff)、2 错误、11 二进制差异、12 相似、13 基于规则的差异、
//! 14 检测到冲突（P7 合并预留）。

use std::io::{IsTerminal, Write as _};
use std::process::ExitCode;

use crate::{aggregate_verdicts, render_dir_diff, render_unified, rules_classify_file, Palette, QuickVerdict, ANSI, PLAIN};
use ai_core::options::{ImportanceOptions, LineAlignAlgo, TextDiffOptions, WhitespaceMode};
use ai_core::LineTable;
use ai_protocol::CompareCriteria;
use ai_vfs::{LocalFs, VfsPath, VfsProvider as _};

use crate::{EXIT_DIFF, EXIT_ERROR, EXIT_SAME};

/// CLI 主入口（aic bin 与 aicc 进程内回退共用）
pub fn run_cli(args: &[String]) -> ExitCode {
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

pub fn print_usage() {
    eprintln!(
        "aic —— ai-compare 命令行\n\
         \n\
         用法:\n  \
         aic diff <left> <right> [--algo standard|myers|patience|histogram]\n  \
                [-w|--ignore-whitespace] [--ignore-whitespace-amount] [--ignore-case]\n  \
                [--compare-eol] [-U <n>] [--color auto|always|never]\n  \
                （两侧为目录时递归输出，diff -r 风格）\n  \
         aic quick <left> <right> [--mode size|crc|binary|rules] [--ignore-whitespace] [--ignore-case]\n\
         \n\
         退出码: 0 相同/成功 / 1 不同 / 2 错误 / 11 二进制差异 / 12 相似（仅不重要差异）\n\
                 13 基于规则的差异 / 14 冲突（三向合并 P7 预留）\n\
         说明: quick --mode rules 走 ai-core 重要性判等（FR-TXT-010）；--ignore-whitespace/--ignore-case\n\
               把对应方面标记为不重要（仅有不重要差异 -> 12 相似），二进制文件回退二进制判等\n\
         目录 quick: size=仅大小; crc=分块哈希; binary=逐字节; rules=逐文件规则判定后聚合"
    );
}

// ---------------------------------------------------------------------------
// aic diff
// ---------------------------------------------------------------------------

/// 文本 diff 选项的 CLI 映射结果
#[derive(Debug, Clone)]
pub struct DiffFlags {
    pub context: u32,
    pub color: String,
    pub opts: TextDiffOptions,
}

impl Default for DiffFlags {
    fn default() -> Self {
        DiffFlags {
            context: 3,
            color: "auto".to_string(),
            opts: TextDiffOptions::default(),
        }
    }
}

fn parse_algo(s: &str) -> Option<LineAlignAlgo> {
    match s {
        "standard" => Some(LineAlignAlgo::Standard),
        "myers" => Some(LineAlignAlgo::Myers),
        "patience" => Some(LineAlignAlgo::Patience),
        "histogram" => Some(LineAlignAlgo::Histogram),
        _ => None,
    }
}

/// 解析 diff 公共开关（aic diff 与 aicc 透传共用；未知开关返回 Err）
pub fn parse_diff_flags(args: &[String]) -> Result<(Vec<&String>, DiffFlags), String> {
    let mut positional: Vec<&String> = Vec::new();
    let mut flags = DiffFlags::default();
    let mut i = 0usize;
    while i < args.len() {
        let a = args[i].as_str();
        match a {
            "-U" | "--unified" => {
                i += 1;
                match args.get(i).and_then(|v| v.parse::<u32>().ok()) {
                    Some(n) => flags.context = n,
                    None => return Err("-U 需要数字参数".into()),
                }
            }
            _ if a.starts_with("-U") && a.len() > 2 => match a[2..].parse::<u32>() {
                Ok(n) => flags.context = n,
                Err(_) => return Err(format!("非法 -U 值: {a}")),
            },
            "--algo" | "--algorithm" => {
                i += 1;
                match args.get(i).map(String::as_str).and_then(parse_algo) {
                    Some(algo) => flags.opts.algo = algo,
                    None => return Err("--algo 仅支持 standard|myers|patience|histogram".into()),
                }
            }
            _ if a.starts_with("--algo=") => {
                match parse_algo(&a["--algo=".len()..]) {
                    Some(algo) => flags.opts.algo = algo,
                    None => return Err("--algo 仅支持 standard|myers|patience|histogram".into()),
                }
            }
            "-w" | "--ignore-whitespace" => flags.opts.whitespace = WhitespaceMode::IgnoreAll,
            "--ignore-whitespace-amount" => flags.opts.whitespace = WhitespaceMode::IgnoreAmount,
            "--ignore-case" | "-i" => flags.opts.ignore_case = true,
            // 行尾风格：默认忽略（docs/03 §6.1）；--compare-eol 显式参与比较
            "--compare-eol" => flags.opts.ignore_eol_style = false,
            "--ignore-eol-style" => flags.opts.ignore_eol_style = true,
            "--color" => {
                i += 1;
                match args.get(i).map(String::as_str) {
                    Some(c @ ("auto" | "always" | "never")) => flags.color = c.to_string(),
                    _ => return Err("--color 仅支持 auto|always|never".into()),
                }
            }
            _ if a.starts_with('-') => return Err(format!("未知参数: {a}")),
            _ => positional.push(&args[i]),
        }
        i += 1;
    }
    Ok((positional, flags))
}

pub fn cmd_diff(args: &[String]) -> ExitCode {
    let (positional, flags) = match parse_diff_flags(args) {
        Ok(v) => v,
        Err(msg) => return arg_error(&msg),
    };
    if positional.len() != 2 {
        return arg_error("diff 需要恰好两个路径参数: <left> <right>");
    }

    let palette: &Palette = match flags.color.as_str() {
        "always" => &ANSI,
        "never" => &PLAIN,
        _ if std::io::stdout().is_terminal() => &ANSI,
        _ => &PLAIN,
    };

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio 运行时");
    let result = rt.block_on(run_diff(
        positional[0],
        positional[1],
        flags.context,
        &flags.opts,
        palette,
    ));
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
    opts: &TextDiffOptions,
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
    let out = ai_core::diff_text(&lt, &rt, opts)
        .map_err(|e| DiffOutcome::Error(e.to_string()))?;

    if out.sections.iter().all(|s| s.kind == ai_core::SectionKind::Equal) {
        return Ok(()); // 相同：无输出，退出码 0
    }
    let text = render_unified(left_s, right_s, &out, &lt, &rt, context, palette);
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
    let text = render_dir_diff(left_s, right_s, &result.arena);
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
pub enum QuickMode {
    Size,
    Crc,
    Binary,
    /// 基于规则的比较（P3 接入 ai-core 重要性判等，替换 P2 二进制占位）
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

pub fn cmd_quick(args: &[String]) -> ExitCode {
    let mut positional: Vec<&String> = Vec::new();
    let mut mode = QuickMode::Crc;
    // rules 模式的重要性开关（/iu 语义子集；映射 FR-TXT-010 重要性模型）
    let mut imp = ImportanceOptions::default();
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
            "--mode=size" => mode = QuickMode::Size,
            "--mode=crc" => mode = QuickMode::Crc,
            "--mode=binary" => mode = QuickMode::Binary,
            "--mode=rules" => mode = QuickMode::Rules,
            "-w" | "--ignore-whitespace" => {
                imp.leading_ws_unimportant = true;
                imp.embedded_ws_unimportant = true;
                imp.trailing_ws_unimportant = true;
            }
            "--ignore-case" | "-i" => imp.case_unimportant = true,
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
    match rt.block_on(run_quick(positional[0], positional[1], mode, &imp)) {
        Ok(verdict) => verdict.exit_code().into(),
        Err(msg) => {
            eprintln!("aic quick: {msg}");
            EXIT_ERROR.into()
        }
    }
}

async fn run_quick(
    left_s: &str,
    right_s: &str,
    mode: QuickMode,
    imp: &ImportanceOptions,
) -> Result<QuickVerdict, String> {
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
        // crc/binary/rules = 内容比较（目录判等内部分块哈希）
        let criteria = match mode {
            QuickMode::Size => CompareCriteria {
                mtime_tolerance_sec: u32::MAX,
                content_compare: ai_protocol::ContentCompareMode::Never,
                ..Default::default()
            },
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
        let stats = result.stats;
        let base_same = stats.content_diff == 0
            && stats.quick_diff == 0
            && stats.left_only == 0
            && stats.right_only == 0;
        let verdict = if base_same {
            QuickVerdict::Same
        } else if mode == QuickMode::Rules {
            // rules：对每个内容差异文件做规则判定后聚合（重要差异 > 二进制差异 > 相似）
            let mut parts = Vec::new();
            for n in result.arena.iter().skip(1) {
                if (n.status == ai_protocol::DirEntryStatus::ContentDiff
                    || n.status == ai_protocol::DirEntryStatus::QuickDiff)
                    && n.kind == ai_protocol::EntryKind::File
                {
                    let lp = left.join(&n.rel_path);
                    let rp = right.join(&n.rel_path);
                    parts.push(rules_classify_file(&vfs, &lp, &rp, imp).await?);
                } else if n.status == ai_protocol::DirEntryStatus::LeftOnly
                    || n.status == ai_protocol::DirEntryStatus::RightOnly
                {
                    // 孤立项：重要差异（对侧缺失）
                    parts.push(QuickVerdict::RulesDiff);
                }
            }
            if parts.is_empty() {
                QuickVerdict::BinaryDiff
            } else {
                aggregate_verdicts(parts)
            }
        } else {
            QuickVerdict::BinaryDiff
        };
        println!(
            "{}: {left_s} {right_s} (mode={}, rows={}, contentDiff={}, orphans=L{}/R{})",
            verdict.as_str(),
            mode.as_str(),
            stats.total,
            stats.content_diff,
            stats.left_only,
            stats.right_only,
        );
        Ok(verdict)
    } else {
        // 文件 quick
        let verdict = match mode {
            QuickMode::Size => {
                if ls.size == rs.size {
                    QuickVerdict::Same
                } else {
                    QuickVerdict::BinaryDiff
                }
            }
            QuickMode::Crc => {
                if ls.size == rs.size
                    && ai_dir::content_same(&vfs, &left, &right, ai_dir::ContentHashMode::Crc)
                        .await
                        .map_err(|e| e.to_string())?
                {
                    QuickVerdict::Same
                } else {
                    QuickVerdict::BinaryDiff
                }
            }
            // binary：逐字节判等（crc 为分块哈希，字节级结果一致；语义上 binary 更严格直读）
            QuickMode::Binary => {
                if ls.size == rs.size
                    && ai_dir::content_same(&vfs, &left, &right, ai_dir::ContentHashMode::Binary)
                        .await
                        .map_err(|e| e.to_string())?
                {
                    QuickVerdict::Same
                } else {
                    QuickVerdict::BinaryDiff
                }
            }
            // P3：rules 接入 ai-core 重要性判等（替换 P2 二进制占位）
            QuickMode::Rules => rules_classify_file(&vfs, &left, &right, imp).await?,
        };
        println!(
            "{}: {left_s} {right_s} (mode={}, size={}B)",
            verdict.as_str(),
            mode.as_str(),
            ls.size
        );
        Ok(verdict)
    }
}

pub fn arg_error(msg: &str) -> ExitCode {
    eprintln!("aic: {msg}");
    print_usage();
    EXIT_ERROR.into()
}
