//! `aic` 命令实现（CLI 全集，docs/04 §P3 / docs/01 §4.8）：
//!
//! - [`run_cli`]：aic 主入口（diff / merge / quick / help / version）；
//! - `aicc`（bin/aicc.rs）复用本模块作为进程内回退路径。
//!
//! 命令面：
//! ```text
//! aic diff <left> <right> [--algo standard|myers|patience|histogram]
//!          [-w|--ignore-whitespace] [--ignore-whitespace-amount]
//!          [--ignore-case] [--compare-eol] [-U <n>] [--color auto|always|never]
//!          （两侧为目录时递归输出，diff -r 风格）
//! aic merge <base> <left> <right> [-o <out>] [--auto|--marker] [--diff3]
//!           [--conflict-exit-code <n>] [--favor-left|--favor-right]
//!           [-iu|--ignore-unimportant] [--on-conflict <策略>] [--conflict-window <n>]
//! aic quick <left> <right> [--mode size|crc|binary|rules] [--ignore-whitespace] [--ignore-case]
//! ```
//!
//! 退出码（FR-CLI-004 兼容分层，见 lib.rs 常量文档）：
//! 0 相同/成功、1 不同(diff)/有未决冲突(merge)、2 错误、11 二进制差异、12 相似、
//! 13 基于规则的差异、14 检测到冲突（aic merge --conflict-exit-code 14 可选）。

use std::io::{IsTerminal, Write as _};
use std::process::ExitCode;

use crate::{
    aggregate_verdicts, render_dir_diff, render_unified, rules_classify_file, verdict_exit_code,
    verdict_label, Palette, QuickVerdict, ANSI, PLAIN,
};
use ai_core::merge::{ConflictDefault, FavorSide, MarkerOptions, MarkerStyle, MergeOptions, SynthesizeOptions, UnresolvedPolicy};
use ai_core::options::{ImportanceOptions, LineAlignAlgo, TextDiffOptions, WhitespaceMode};
use ai_core::LineTable;
use ai_protocol::CompareCriteria;
use ai_vfs::{RouteFs, VfsPath, VfsProvider as _};

use crate::{EXIT_DIFF, EXIT_ERROR, EXIT_SAME};

/// CLI 主入口（aic bin 与 aicc 进程内回退共用）
pub fn run_cli(args: &[String]) -> ExitCode {
    match args.first().map(String::as_str) {
        Some("diff") => cmd_diff(&args[1..]),
        Some("report") => crate::report_cmd::cmd_report(&args[1..]),
        // FR-CLI-002 位置参数形态：aic @script.aicscript [args...]
        Some(s) if s.starts_with('@') => {
            let mut script_args = vec![s[1..].to_string()];
            script_args.extend(args[1..].iter().cloned());
            crate::script::run_cli_script(&script_args)
        }
        Some("script") => crate::script::cmd_script(&args[1..]),
        Some("merge") => cmd_merge(&args[1..]),
        Some("quick") => cmd_quick(&args[1..]),
        Some("table") => cmd_table(&args[1..]),
        Some("hex") => cmd_hex(&args[1..]),
        Some("picture") => cmd_picture(&args[1..]),
        Some("patch") => cmd_patch(&args[1..]),
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
                [--rules <rules.json>] [--format-load <formats.bcpack>]\n  \
                （两侧为目录时递归输出，diff -r 风格；--rules 加载规则文档全局层，\n\
                  --format-load 加载格式集——两者加载即生效，重要性规则随文件格式\n\
                  语法折叠注释/字符串差异，对齐骨架不变）\n  \
         aic merge <base> <left> <right> [-o <out>] [--auto|--marker] [--diff3]\n  \
                [--conflict-exit-code <n>] [--favor-left|--favor-right]\n  \
                [-iu|--ignore-unimportant] [--on-conflict left|right|base|leftThenRight|rightThenLeft]\n  \
                [--conflict-window <n>] [-w] [--ignore-case] [--compare-eol]\n  \
                （三向合并：退出 0 无冲突 / 1 有未决冲突 / 2 错误；marker 模式以\n\
                  <<<<<<< / ======= / >>>>>>> 标记冲突，--diff3 附 ||||||| base 段）\n  \
         aic quick <left> <right> [--mode size|crc|binary|rules] [--ignore-whitespace] [--ignore-case]\n  \
         aic report <left> <right> --layout summary|side-by-side|interleaved|patch|html|xml|csv|tree|list|stats\n  \
                [-o <out>] [--title <t>] [--display all|mismatches|context|matches] [--dir-display <f>]\n  \
                [--ignore-unimportant] [--line-numbers] [-U <n>] [--patch-style unified|context|normal]\n  \
                [--tsv] [--color|--mono] [--kind auto|dir|text|table|hex] [--file-links]\n  \
                （文件夹/文本/表格/十六进制报告；-o 支持 %f/%F/%k/%t 文件名模板占位符）\n  \
         aic script run <file.aicscript> [args...] [--var name=value] [--silent]\n  \
                （FR-SCR 脚本处理器；退出码 105 读失败/106 语法错误/107 加载失败；\n\
                  aiscript bin 为同引擎独立入口；aic @script.txt 为位置参数形态）\n\
         \n\
         退出码: 0 相同/成功 / 1 不同 / 2 错误 / 11 二进制差异 / 12 相似（仅不重要差异）\n\
                 13 基于规则的差异 / 14 冲突（aic merge --conflict-exit-code 14 对齐 FR-CLI-004）\n\
         说明: quick --mode rules 走 ai-core 重要性判等（FR-TXT-010）；--ignore-whitespace/--ignore-case\n\
               把对应方面标记为不重要（仅有不重要差异 -> 12 相似），二进制文件回退二进制判等\n\
         目录 quick: size=仅大小; crc=分块哈希; binary=逐字节; rules=逐文件规则判定后聚合\n\
         merge 说明: 默认 marker 模式（冲突写标记供审查）；--auto 输出自动合成文本，\n\
               未决冲突按 --on-conflict 策略解析（默认 left，受惠侧语义 /favorleft）；\n\
               -iu/--ignore-unimportant + --favor-*：不重要冲突自动从受惠侧采用（FR-TMRG-005）"
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
    /// P10：规则文档（rules.json 形态；加载即生效，全局层）
    pub rules_path: Option<String>,
    /// P10：格式集（.bcpack / JSON；加载后参与文件格式检测，语法供规则折叠）
    pub format_pack_path: Option<String>,
}

impl Default for DiffFlags {
    fn default() -> Self {
        DiffFlags {
            context: 3,
            color: "auto".to_string(),
            opts: TextDiffOptions::default(),
            rules_path: None,
            format_pack_path: None,
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
            // P10：规则/格式集加载即生效
            "--rules" => {
                i += 1;
                match args.get(i) {
                    Some(p) => flags.rules_path = Some(p.to_string()),
                    None => return Err("--rules 需要规则文档路径（rules.json 形态）".into()),
                }
            }
            _ if a.starts_with("--rules=") => {
                flags.rules_path = Some(a["--rules=".len()..].to_string());
            }
            "--format-load" => {
                i += 1;
                match args.get(i) {
                    Some(p) => flags.format_pack_path = Some(p.to_string()),
                    None => return Err("--format-load 需要格式集路径（.bcpack / JSON）".into()),
                }
            }
            _ if a.starts_with("--format-load=") => {
                flags.format_pack_path = Some(a["--format-load=".len()..].to_string());
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
        &flags,
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
    flags: &DiffFlags,
    palette: &Palette,
) -> Result<(), DiffOutcome> {
    let vfs = RouteFs::new();
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

    // P10：格式集 / 规则文档加载即生效（--format-load 先建注册表，--rules 取全局层；
    // 语法按左侧文件名检测的文件格式提供，未命中 → 不折叠）
    let mut registry = ai_core::FormatRegistry::builtin();
    if let Some(pack_path) = &flags.format_pack_path {
        let text = std::fs::read_to_string(pack_path)
            .map_err(|e| DiffOutcome::Error(format!("格式集读取失败 {pack_path}: {e}")))?;
        let pack = ai_core::FormatPack::from_json(&text)
            .map_err(|e| DiffOutcome::Error(format!("格式集解析失败: {e}")))?;
        registry.merge_pack(&pack);
    }
    let mut config = ai_core::DiffConfig { base: flags.opts.clone(), ..ai_core::DiffConfig::default() };
    if let Some(rules_path) = &flags.rules_path {
        let text = std::fs::read_to_string(rules_path)
            .map_err(|e| DiffOutcome::Error(format!("规则文档读取失败 {rules_path}: {e}")))?;
        let doc = ai_core::RulesDocument::from_json(&text)
            .map_err(|e| DiffOutcome::Error(format!("规则文档解析失败: {e}")))?;
        let set = ai_core::RuleSet::from_document(&doc);
        let fname = std::path::Path::new(left_s)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(left_s);
        let head = vfs.read_range(&left_path, 0, 8192).await.unwrap_or_default();
        let fmt = registry.detect(fname, &head);
        let effective = set.resolve(None, None, fmt.map(|f| f.id.as_str()));
        config.importance = effective.to_importance_options();
        config.base.rules = ai_core::RulesOptions::resolve(&effective, fmt);
    }

    let keyed = ai_core::diff_config(&lt, &rt, &config)
        .map_err(|e| DiffOutcome::Error(e.to_string()))?;
    let out = keyed.output;

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
    vfs: &RouteFs,
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
            importance: Default::default(),
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
// aic merge（P7 三向合并；FR-CLI-003 /automerge /favorleft|right /force 子集）
// ---------------------------------------------------------------------------

/// merge 输出策略的 CLI 形态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MergeOutputKind {
    /// marker 模式（默认；冲突写标记供审查，/force 语义）
    Marker,
    /// auto 模式（--auto；自动合成文本，未决冲突按策略解析）
    Auto,
}

pub fn cmd_merge(args: &[String]) -> ExitCode {
    let mut positional: Vec<&String> = Vec::new();
    let mut opts = MergeOptions::default();
    let mut kind = MergeOutputKind::Marker;
    let mut marker_style = MarkerStyle::Merge;
    let mut out_path: Option<String> = None;
    let mut conflict_exit_code: u8 = 1;
    let mut diff_opts = TextDiffOptions::default();

    let mut i = 0usize;
    while i < args.len() {
        let a = args[i].as_str();
        let next = |i: &mut usize| -> Option<String> {
            *i += 1;
            args.get(*i).cloned()
        };
        match a {
            "-o" | "--output" | "--mergeoutput" => match next(&mut i) {
                Some(p) => out_path = Some(p),
                None => return arg_error("-o 需要输出路径参数"),
            },
            _ if a.starts_with("--output=") => out_path = Some(a["--output=".len()..].to_string()),
            "--auto" | "--automerge" => kind = MergeOutputKind::Auto,
            "--marker" => kind = MergeOutputKind::Marker,
            "--diff3" => marker_style = MarkerStyle::Diff3,
            "--conflict-exit-code" => match next(&mut i).and_then(|v| v.parse::<u8>().ok()) {
                // 冲突码不得占用既有语义码，否则 VCS/CI 会把"有冲突"误判为
                // 成功(0)/错误(2)/二进制差异(11)/相似(12)/规则差异(13)。
                // 14 是 FR-CLI-004 的冲突码本身，1 是默认值，均放行。
                Some(n @ (0 | 2 | 11 | 12 | 13)) => {
                    return arg_error(&format!(
                        "--conflict-exit-code 不可为 {n}（该码已占用：0 相同/2 错误/11 二进制差异/12 相似/13 规则差异）"
                    ))
                }
                Some(n) => conflict_exit_code = n,
                None => return arg_error("--conflict-exit-code 需要数字参数（默认 1；14 = FR-CLI-004 冲突码）"),
            },
            "--favor-left" | "--favorleft" => opts.favor = Some(FavorSide::Left),
            "--favor-right" | "--favorright" => opts.favor = Some(FavorSide::Right),
            // /iu 语义：打开忽略不重要差异 = 空白三类/大小写标记为不重要 +
            // 不重要冲突不升级为未决（FR-TMRG-005；行尾默认已不重要）
            "-iu" | "--ignore-unimportant" => {
                opts.ignore_unimportant = true;
                opts.importance.leading_ws_unimportant = true;
                opts.importance.embedded_ws_unimportant = true;
                opts.importance.trailing_ws_unimportant = true;
                opts.importance.case_unimportant = true;
            }
            "--on-conflict" => {
                let v = next(&mut i).unwrap_or_default();
                opts.conflict_default = match v.as_str() {
                    "left" => ConflictDefault::TakeLeft,
                    "right" => ConflictDefault::TakeRight,
                    "base" | "center" => ConflictDefault::TakeBase,
                    "leftThenRight" | "left-then-right" => ConflictDefault::TakeLeftThenRight,
                    "rightThenLeft" | "right-then-left" => ConflictDefault::TakeRightThenLeft,
                    _ => return arg_error("--on-conflict 仅支持 left|right|base|leftThenRight|rightThenLeft"),
                };
            }
            "--conflict-window" => match next(&mut i).and_then(|v| v.parse::<u32>().ok()) {
                Some(n) => opts.conflict_window = Some(n),
                None => return arg_error("--conflict-window 需要数字参数（FR-TMRG-003 N 行分隔）"),
            },
            "-w" | "--ignore-whitespace" => diff_opts.whitespace = WhitespaceMode::IgnoreAll,
            "--ignore-whitespace-amount" => diff_opts.whitespace = WhitespaceMode::IgnoreAmount,
            "--ignore-case" | "-i" => diff_opts.ignore_case = true,
            "--compare-eol" => diff_opts.ignore_eol_style = false,
            _ if a.starts_with('-') => return arg_error(&format!("未知参数: {a}")),
            _ => positional.push(&args[i]),
        }
        i += 1;
    }
    if positional.len() != 3 {
        return arg_error("merge 需要恰好三个路径参数: <base> <left> <right>");
    }
    opts.diff = diff_opts;

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio 运行时");
    let result = rt.block_on(run_merge(
        positional[0],
        positional[1],
        positional[2],
        &opts,
        kind,
        marker_style,
        out_path.as_deref(),
    ));
    match result {
        Ok(()) => EXIT_SAME.into(),
        Err(MergeOutcome::Conflicts) => conflict_exit_code.into(),
        Err(MergeOutcome::Error(msg)) => {
            eprintln!("aic merge: {msg}");
            EXIT_ERROR.into()
        }
    }
}

enum MergeOutcome {
    /// 存在未决冲突（输出已写出）
    Conflicts,
    Error(String),
}

async fn run_merge(
    base_s: &str,
    left_s: &str,
    right_s: &str,
    opts: &MergeOptions,
    kind: MergeOutputKind,
    marker_style: MarkerStyle,
    out_path: Option<&str>,
) -> Result<(), MergeOutcome> {
    let vfs = RouteFs::new();
    let (base_p, left_p, right_p) = (
        VfsPath::parse(base_s).map_err(|e| MergeOutcome::Error(e.to_string()))?,
        VfsPath::parse(left_s).map_err(|e| MergeOutcome::Error(e.to_string()))?,
        VfsPath::parse(right_s).map_err(|e| MergeOutcome::Error(e.to_string()))?,
    );
    let base_text = vfs.read_file_text(&base_p).await.map_err(|e| MergeOutcome::Error(e.to_string()))?;
    let left_text = vfs.read_file_text(&left_p).await.map_err(|e| MergeOutcome::Error(e.to_string()))?;
    let right_text = vfs.read_file_text(&right_p).await.map_err(|e| MergeOutcome::Error(e.to_string()))?;

    let base = LineTable::from_text(&base_text);
    let left = LineTable::from_text(&left_text);
    let right = LineTable::from_text(&right_text);

    let model = ai_core::merge3(&base, &left, &right, opts);
    let unresolved = model.unresolved();

    // 输出策略：marker 模式（默认，/force 语义）or auto 模式（conflict_default 可配）
    let syn = match kind {
        MergeOutputKind::Marker => SynthesizeOptions {
            unresolved: UnresolvedPolicy::Markers(MarkerOptions {
                style: marker_style,
                // 标签 = 路径原串（与 GNU diff3 -m / git merge-file 的 argv 标签同形）
                base_label: base_s.to_string(),
                left_label: left_s.to_string(),
                right_label: right_s.to_string(),
            }),
        },
        MergeOutputKind::Auto => SynthesizeOptions {
            unresolved: opts.conflict_default.clone().to_policy(None),
        },
    };
    let out = ai_core::synthesize(&model, &base, &left, &right, &std::collections::HashMap::new(), &syn);

    match out_path {
        Some(p) => {
            std::fs::write(p, out.text.as_bytes())
                .map_err(|e| MergeOutcome::Error(format!("写输出失败: {e}")))?;
        }
        None => {
            let stdout = std::io::stdout();
            let mut lock = stdout.lock();
            let _ = lock.write_all(out.text.as_bytes());
            let _ = lock.flush();
        }
    }
    if unresolved.is_empty() {
        Ok(())
    } else {
        eprintln!(
            "aic merge: {} 个未决冲突块（{}）",
            unresolved.len(),
            unresolved.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(",")
        );
        Err(MergeOutcome::Conflicts)
    }
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
        Ok(verdict) => verdict_exit_code(verdict).into(),
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
    let vfs = RouteFs::new();
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
            &ai_dir::DirScanOptions {
                depth: None,
                criteria,
                importance: Default::default(),
            },
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
            verdict_label(verdict),
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
            verdict_label(verdict),
            mode.as_str(),
            ls.size
        );
        Ok(verdict)
    }
}

// ---------------------------------------------------------------------------
// aic table（P9 表格比较；FR-TBL 引擎侧 CLI 面）
// ---------------------------------------------------------------------------

pub fn cmd_table(args: &[String]) -> ExitCode {
    let mut positional: Vec<&String> = Vec::new();
    let mut opts = ai_core::table::TableDiffOptions::default();
    let mut i = 0usize;
    while i < args.len() {
        let a = args[i].as_str();
        let next = |i: &mut usize| -> Option<String> {
            *i += 1;
            args.get(*i).cloned()
        };
        match a {
            "--key" => match next(&mut i) {
                Some(spec) => {
                    for part in spec.split(',') {
                        match part.trim().parse::<usize>() {
                            Ok(c) => opts.key_columns.push(c),
                            Err(_) => return arg_error(&format!("--key 列下标非法: {part}")),
                        }
                    }
                }
                None => return arg_error("--key 需要逗号分隔的列下标"),
            },
            _ if a.starts_with("--key=") => {
                for part in a["--key=".len()..].split(',') {
                    match part.trim().parse::<usize>() {
                        Ok(c) => opts.key_columns.push(c),
                        Err(_) => return arg_error(&format!("--key 列下标非法: {part}")),
                    }
                }
            }
            "--delimiter" => match next(&mut i).and_then(|v| v.chars().next()) {
                Some(c) => opts.parse.delimiter = c,
                None => return arg_error("--delimiter 需要单字符参数"),
            },
            _ if a.starts_with("--delimiter=") => {
                match a["--delimiter=".len()..].chars().next() {
                    Some(c) => opts.parse.delimiter = c,
                    None => return arg_error("--delimiter 需要单字符参数"),
                }
            }
            "--tab" | "--tsv" => opts.parse.delimiter = '\t',
            "--no-header" => opts.parse.has_header = false,
            "--align" => match next(&mut i).as_deref() {
                Some("none") => opts.column_align = ai_core::table::ColumnAlign::None,
                Some("names-left" | "left") => opts.column_align = ai_core::table::ColumnAlign::ByLeftName,
                Some("names-right" | "right") => opts.column_align = ai_core::table::ColumnAlign::ByRightName,
                _ => return arg_error("--align 仅支持 none|names-left|names-right"),
            },
            "--rows" => match next(&mut i).as_deref() {
                Some("none" | "unaligned") => opts.row_align = ai_core::table::RowAlign::Unaligned,
                Some("standard") => opts.row_align = ai_core::table::RowAlign::Standard,
                _ => return arg_error("--rows 仅支持 none|standard"),
            },
            "--skew" => match next(&mut i).and_then(|v| v.parse::<u32>().ok()) {
                Some(n) => opts.skew_tolerance = n,
                None => return arg_error("--skew 需要数字参数"),
            },
            // -iu：大小写 + 空白三类不重要（FR-TXT-010 语义复用）
            "-iu" | "--ignore-unimportant" => {
                opts.importance.case_unimportant = true;
                opts.importance.whitespace = ai_core::options::WhitespaceMode::IgnoreAmount;
            }
            "-w" | "--ignore-whitespace" => opts.importance.whitespace = ai_core::options::WhitespaceMode::IgnoreAll,
            "--ignore-case" | "-i" => opts.importance.case_unimportant = true,
            _ if a.starts_with('-') => return arg_error(&format!("未知参数: {a}")),
            _ => positional.push(&args[i]),
        }
        i += 1;
    }
    if positional.len() != 2 {
        return arg_error("table 需要恰好两个路径参数: <left> <right>");
    }

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio 运行时");
    match rt.block_on(run_table(positional[0], positional[1], &opts)) {
        Ok(()) => EXIT_SAME.into(),
        Err(TableOutcome::Different) => EXIT_DIFF.into(),
        Err(TableOutcome::Error(msg)) => {
            eprintln!("aic table: {msg}");
            EXIT_ERROR.into()
        }
    }
}

enum TableOutcome {
    Different,
    Error(String),
}

async fn run_table(left_s: &str, right_s: &str, opts: &ai_core::table::TableDiffOptions) -> Result<(), TableOutcome> {
    let vfs = RouteFs::new();
    let left_path = VfsPath::parse(left_s).map_err(|e| TableOutcome::Error(e.to_string()))?;
    let right_path = VfsPath::parse(right_s).map_err(|e| TableOutcome::Error(e.to_string()))?;
    let left_text = vfs.read_file_text(&left_path).await.map_err(|e| TableOutcome::Error(e.to_string()))?;
    let right_text = vfs.read_file_text(&right_path).await.map_err(|e| TableOutcome::Error(e.to_string()))?;

    let model = ai_core::table::diff_tables(&left_text, &right_text, opts)
        .map_err(|e| TableOutcome::Error(e.to_string()))?;

    use std::io::Write as _;
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    let s = &model.summary;
    if s.changed_rows == 0 && s.added_rows == 0 && s.removed_rows == 0 {
        return Ok(()); // 相同：无输出，退出码 0
    }
    // 摘要头 + TSV 差异行
    let _ = writeln!(
        lock,
        "# same={}\tchanged={}\tadded={}\tremoved={}\tunimportant={}",
        s.same_rows, s.changed_rows, s.added_rows, s.removed_rows, s.unimportant_rows
    );
    let cell_mark = |st: ai_core::table::CellDiffStatus| match st {
        ai_core::table::CellDiffStatus::Same => '=',
        ai_core::table::CellDiffStatus::UnimportantDiff => '~',
        ai_core::table::CellDiffStatus::ImportantDiff => '!',
        ai_core::table::CellDiffStatus::LeftOnly => '<',
        ai_core::table::CellDiffStatus::RightOnly => '>',
    };
    for row in &model.rows {
        let cells: Vec<String> = row
            .cells
            .iter()
            .map(|c| {
                let l = c.left.clone().unwrap_or_default();
                let r = c.right.clone().unwrap_or_default();
                format!("{}{}|{}", cell_mark(c.status), escape_cell(&l), escape_cell(&r))
            })
            .collect();
        let _ = writeln!(
            lock,
            "{}\t{}\t{}\t{}",
            row.kind.as_str(),
            row.left.map(|v| v.to_string()).unwrap_or_else(|| "-".into()),
            row.right.map(|v| v.to_string()).unwrap_or_else(|| "-".into()),
            cells.join("\t")
        );
    }
    let _ = lock.flush();
    Err(TableOutcome::Different)
}

fn escape_cell(v: &str) -> String {
    v.replace('\\', "\\\\").replace('\t', "\\t").replace('\n', "\\n").replace('\r', "\\r")
}

// ---------------------------------------------------------------------------
// aic picture <l.png> <r.png> [--tolerance n] [--block n]\n  aic hex（P9 十六进制比较；FR-HEX 引擎侧 CLI 面）
// ---------------------------------------------------------------------------

pub fn cmd_hex(args: &[String]) -> ExitCode {
    let mut positional: Vec<&String> = Vec::new();
    let mut opts = ai_core::hex::HexOptions::default();
    let mut i = 0usize;
    while i < args.len() {
        let a = args[i].as_str();
        let next = |i: &mut usize| -> Option<String> {
            *i += 1;
            args.get(*i).cloned()
        };
        match a {
            "--chunk" => match next(&mut i).and_then(|v| v.parse::<usize>().ok()) {
                Some(n) if n > 0 => opts.chunk_size = n,
                _ => return arg_error("--chunk 需要正整数参数"),
            },
            _ if a.starts_with("--chunk=") => match a["--chunk=".len()..].parse::<usize>() {
                Ok(n) if n > 0 => opts.chunk_size = n,
                _ => return arg_error("--chunk 需要正整数参数"),
            },
            "--align" => match next(&mut i).as_deref() {
                Some("full") => opts.align_mode = ai_core::hex::HexAlignMode::Full,
                Some("fast") => opts.align_mode = ai_core::hex::HexAlignMode::Fast,
                Some("none") => opts.align_mode = ai_core::hex::HexAlignMode::None,
                _ => return arg_error("--align 仅支持 full|fast|none"),
            },
            _ if a.starts_with('-') => return arg_error(&format!("未知参数: {a}")),
            _ => positional.push(&args[i]),
        }
        i += 1;
    }
    if positional.len() != 2 {
        return arg_error("hex 需要恰好两个路径参数: <left> <right>");
    }

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio 运行时");
    match rt.block_on(run_hex(positional[0], positional[1], &opts)) {
        Ok(()) => EXIT_SAME.into(),
        Err(HexOutcome::Different) => EXIT_DIFF.into(),
        Err(HexOutcome::Error(msg)) => {
            eprintln!("aic hex: {msg}");
            EXIT_ERROR.into()
        }
    }
}

enum HexOutcome {
    Different,
    Error(String),
}

async fn run_hex(left_s: &str, right_s: &str, opts: &ai_core::hex::HexOptions) -> Result<(), HexOutcome> {
    let vfs = RouteFs::new();
    let left_path = VfsPath::parse(left_s).map_err(|e| HexOutcome::Error(e.to_string()))?;
    let right_path = VfsPath::parse(right_s).map_err(|e| HexOutcome::Error(e.to_string()))?;
    let left = vfs.read_file_bytes(&left_path).await.map_err(|e| HexOutcome::Error(e.to_string()))?;
    let right = vfs.read_file_bytes(&right_path).await.map_err(|e| HexOutcome::Error(e.to_string()))?;

    let model = ai_core::hex::diff_bytes(&left, &right, opts);
    use std::io::Write as _;
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    if model.blocks.iter().all(|b| b.kind == ai_core::hex::HexBlockKind::Same) {
        return Ok(());
    }
    let sum = &model.summary;
    let _ = writeln!(
        lock,
        "# same={}B\tchanged={}B\tinserted={}B\tdeleted={}B\tblocks={}",
        sum.same_bytes, sum.changed_bytes, sum.inserted_bytes, sum.deleted_bytes, model.blocks.len()
    );
    let kind = |b: ai_core::hex::HexBlockKind| match b {
        ai_core::hex::HexBlockKind::Same => "same",
        ai_core::hex::HexBlockKind::Changed => "changed",
        ai_core::hex::HexBlockKind::Inserted => "inserted",
        ai_core::hex::HexBlockKind::Deleted => "deleted",
    };
    for b in &model.blocks {
        let _ = match (b.left, b.right) {
            (Some(l), Some(r)) => writeln!(
                lock,
                "{}\tleft[0x{:x}..0x{:x})\tright[0x{:x}..0x{:x})\t{}B",
                kind(b.kind), l.offset, l.offset + l.len, r.offset, r.offset + r.len, l.len
            ),
            (Some(l), None) => writeln!(lock, "{}\tleft[0x{:x}..0x{:x})\t{}B", kind(b.kind), l.offset, l.offset + l.len, l.len),
            (None, Some(r)) => writeln!(lock, "{}\tright[0x{:x}..0x{:x})\t{}B", kind(b.kind), r.offset, r.offset + r.len, r.len),
            (None, None) => Ok(()),
        };
    }
    // 首个差异区域的十六进制行预览（每侧至多 4 行）
    for b in model.blocks.iter().filter(|b| b.kind != ai_core::hex::HexBlockKind::Same).take(1) {
        if let Some(l) = b.left {
            let _ = writeln!(lock, "-- left preview @0x{:x}", l.offset);
            let off = (l.offset as usize / ai_core::hex::HEX_LINE_BYTES) * ai_core::hex::HEX_LINE_BYTES;
            for line in ai_core::hex::render_hex_lines(&left, (off / ai_core::hex::HEX_LINE_BYTES) as u64, 4) {
                let _ = writeln!(lock, "{}  {}  {}", line.addr, line.hex, line.ascii);
            }
        }
        if let Some(r) = b.right {
            let _ = writeln!(lock, "-- right preview @0x{:x}", r.offset);
            let off = (r.offset as usize / ai_core::hex::HEX_LINE_BYTES) * ai_core::hex::HEX_LINE_BYTES;
            for line in ai_core::hex::render_hex_lines(&right, (off / ai_core::hex::HEX_LINE_BYTES) as u64, 4) {
                let _ = writeln!(lock, "{}  {}  {}", line.addr, line.hex, line.ascii);
            }
        }
    }
    let _ = lock.flush();
    Err(HexOutcome::Different)
}

// ---------------------------------------------------------------------------
// aic patch apply（P9 补丁应用；FR-PATCH-011 CLI 面）
// ---------------------------------------------------------------------------

pub fn cmd_patch(args: &[String]) -> ExitCode {
    let Some(sub) = args.first().map(String::as_str) else {
        print_usage();
        return EXIT_ERROR.into();
    };
    if sub != "apply" {
        eprintln!("aic patch: 未知子命令: {sub}（仅支持 apply）");
        print_usage();
        return EXIT_ERROR.into();
    }
    let mut positional: Vec<&String> = Vec::new();
    let mut input: Option<String> = None;
    let mut output: Option<String> = None;
    let mut dir: Option<String> = None;
    let mut opts = ai_core::patch::PatchApplyOptions::default();
    let mut i = 1usize;
    while i < args.len() {
        let a = args[i].as_str();
        let next = |i: &mut usize| -> Option<String> {
            *i += 1;
            args.get(*i).cloned()
        };
        match a {
            "--input" | "-i" => match next(&mut i) {
                Some(v) => input = Some(v),
                None => return arg_error("--input 需要目标文件参数"),
            },
            _ if a.starts_with("--input=") => input = Some(a["--input=".len()..].to_string()),
            "-o" | "--output" => match next(&mut i) {
                Some(v) => output = Some(v),
                None => return arg_error("-o 需要输出路径参数"),
            },
            _ if a.starts_with("--output=") => output = Some(a["--output=".len()..].to_string()),
            "--dir" | "--root" => match next(&mut i) {
                Some(v) => dir = Some(v),
                None => return arg_error("--dir 需要目标根目录参数"),
            },
            _ if a.starts_with("--dir=") => dir = Some(a["--dir=".len()..].to_string()),
            "--dry-run" => opts.dry_run = true,
            "--fuzz" => match next(&mut i).and_then(|v| v.parse::<u32>().ok()) {
                Some(n) => opts.fuzz = n,
                None => return arg_error("--fuzz 需要数字参数（0 = 仅精确匹配）"),
            },
            "--no-crlf-tolerance" => opts.crlf_tolerant = false,
            _ if a.starts_with('-') => return arg_error(&format!("未知参数: {a}")),
            _ => positional.push(&args[i]),
        }
        i += 1;
    }
    let Some(patch_path) = positional.first() else {
        return arg_error("patch apply 需要 <target.patch> 参数");
    };
    let patch_text = match std::fs::read_to_string(patch_path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("aic patch: 读补丁失败 {patch_path}: {e}");
            return EXIT_ERROR.into();
        }
    };

    use std::io::Write as _;
    if let Some(input_path) = &input {
        // 单文件模式：补丁须恰含一个文件目标
        let parsed = match ai_core::patch::parse_patch(&patch_text) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("aic patch: {e}");
                return EXIT_ERROR.into();
            }
        };
        if parsed.files.len() != 1 {
            eprintln!(
                "aic patch: 补丁含 {} 个文件目标——单文件模式请用 --input 配单文件补丁，多文件用 --dir",
                parsed.files.len()
            );
            return EXIT_ERROR.into();
        }
        let orig = match std::fs::read_to_string(input_path) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("aic patch: 读目标失败 {input_path}: {e}");
                return EXIT_ERROR.into();
            }
        };
        let file = &parsed.files[0];
        if file.binary {
            eprintln!("aic patch: 二进制文件跳过（补丁标注 Binary/GIT binary patch）: {}", file.label());
            return EXIT_ERROR.into();
        }
        return match ai_core::patch::apply_file(&orig, file, &opts) {
            Ok(ai_core::patch::ApplyOutcome::Applied { text, fuzz_used, offset }) => {
                if opts.dry_run {
                    eprintln!("aic patch: DRY OK（fuzz={fuzz_used}, offset={offset}）");
                    EXIT_SAME.into()
                } else {
                    match output.as_deref() {
                        Some(out) => {
                            if let Err(e) = std::fs::write(out, text.as_bytes()) {
                                eprintln!("aic patch: 写输出失败 {out}: {e}");
                                EXIT_ERROR.into()
                            } else {
                                EXIT_SAME.into()
                            }
                        }
                        None => {
                            let stdout = std::io::stdout();
                            let mut lock = stdout.lock();
                            let _ = lock.write_all(text.as_bytes());
                            let _ = lock.flush();
                            EXIT_SAME.into()
                        }
                    }
                }
            }
            Ok(ai_core::patch::ApplyOutcome::NoOp) => {
                eprintln!("aic patch: 无内容 hunk（纯元数据变更）");
                EXIT_SAME.into()
            }
            Err(msg) => {
                // 补丁应用失败属"错误"（上下文不匹配/文件不可写），不是"文件不同"：
                // 退出码契约 1 = 不同、2 = 错误（见 lib.rs）。返回 1 会让 CI
                // 把一次失败的补丁当成"存在差异"。
                eprintln!("aic patch: 应用失败: {msg}");
                EXIT_ERROR.into()
            }
        };
    }
    // 多文件模式（--dir）
    let Some(root) = &dir else {
        return arg_error("patch apply 需要 --input <file>（单文件）或 --dir <root>（多文件）");
    };
    match ai_core::patch::apply_patch(&ai_core::patch::StdFs, std::path::Path::new(root), &patch_text, &opts) {
        Ok(report) => {
            use std::io::Write as _;
            let stdout = std::io::stdout();
            let mut lock = stdout.lock();
            for r in &report.results {
                let status = match r.status {
                    ai_core::patch::FileApplyStatus::Applied => "applied",
                    ai_core::patch::FileApplyStatus::AlreadyApplied => "alreadyApplied",
                    ai_core::patch::FileApplyStatus::SkippedBinary => "skippedBinary",
                    ai_core::patch::FileApplyStatus::NoOp => "noOp",
                    ai_core::patch::FileApplyStatus::Failed => "FAILED",
                };
                let _ = writeln!(lock, "{}\t{}\tfuzz={}\toffset={}\t{}", status, r.path, r.fuzz_used, r.offset_applied, r.detail.clone().unwrap_or_default());
            }
            let _ = lock.flush();
            // 退出码语义（lib.rs 契约）：patch apply 是"执行"而非"比较"，
            // 没有"文件不同"这一结果。
            // - 有文件失败（Failed）或已应用（AlreadyApplied，git apply 同语义拒绝）→ 2 错误
            // - 全部成功（Applied/NoOp）或仅二进制跳过（SkippedBinary，设计语义）→ 0 成功
            let rejected = report.results.iter().any(|r| {
                matches!(
                    r.status,
                    ai_core::patch::FileApplyStatus::Failed
                        | ai_core::patch::FileApplyStatus::AlreadyApplied
                )
            });
            if rejected {
                EXIT_ERROR.into()
            } else {
                EXIT_SAME.into()
            }
        }
        Err(e) => {
            eprintln!("aic patch: {e}");
            EXIT_ERROR.into()
        }
    }
}

pub fn arg_error(msg: &str) -> ExitCode {
    eprintln!("aic: {msg}");
    print_usage();
    EXIT_ERROR.into()
}

// aic picture
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default)]
pub struct PictureFlags {
    pub tolerance: Option<u16>,
    pub block: Option<u32>,
}

pub fn cmd_picture(args: &[String]) -> ExitCode {
    let mut positional = Vec::new();
    let mut flags = PictureFlags::default();
    let mut i = 0usize;
    while i < args.len() {
        match args[i].as_str() {
            "--tolerance" => {
                i += 1;
                if let Some(v) = args.get(i).and_then(|s| s.parse::<u16>().ok()) {
                    flags.tolerance = Some(v.min(255));
                }
            }
            "--block" => {
                i += 1;
                if let Some(v) = args.get(i).and_then(|s| s.parse::<u32>().ok()) {
                    flags.block = Some(v.max(1));
                }
            }
            other => positional.push(other.to_string()),
        }
        i += 1;
    }
    if positional.len() != 2 {
        eprintln!("aic picture <left.png> <right.png> [--tolerance n] [--block n]");
        return ExitCode::from(2);
    }
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio 运行时");
    match rt.block_on(run_picture(&positional[0], &positional[1], &flags)) {
        Ok(same) if same => ExitCode::from(0),
        Ok(_) => ExitCode::from(1),
        Err(msg) => {
            eprintln!("aic picture: {msg}");
            ExitCode::from(2)
        }
    }
}

async fn run_picture(left_s: &str, right_s: &str, flags: &PictureFlags) -> Result<bool, String> {
    let vfs = RouteFs::new();
    let left_path = VfsPath::parse(left_s).map_err(|e| e.to_string())?;
    let right_path = VfsPath::parse(right_s).map_err(|e| e.to_string())?;
    let left = vfs.read_file_bytes(&left_path).await.map_err(|e| e.to_string())?;
    let right = vfs.read_file_bytes(&right_path).await.map_err(|e| e.to_string())?;
    let opts = ai_core::picture::PictureDiffOptions {
        tolerance: flags.tolerance.unwrap_or(8) as u8,
        block: flags.block.unwrap_or(32),
    };
    let r = ai_core::picture::diff_bytes(&left, &right, &opts)?;
    let same = r.diff_pixels == 0;
    use std::io::Write as _;
    let mut lock = std::io::stdout().lock();
    if same {
        let _ = writeln!(lock, "same: {left_s} {right_s} (similarity=100%)");
        return Ok(true);
    }
    let _ = writeln!(
        lock,
        "# diff\tpixels={}\ttotal={}\tsimilarity={:.2}%\tblocks={}\tcanvas={}x{}\tleft={}x{}\tright={}x{}",
        r.diff_pixels, r.total_pixels, r.similarity_pct, r.blocks.len(),
        r.width, r.height, r.left_size.0, r.left_size.1, r.right_size.0, r.right_size.1
    );
    for b in r.blocks.iter().take(20) {
        let _ = writeln!(
            lock,
            "block ({},{}) {}x{} diff {:.0}%",
            b.x, b.y, b.w, b.h, b.diff_ratio * 100.0
        );
    }
    if r.blocks.len() > 20 {
        let _ = writeln!(lock, "... 其余 {} 块省略", r.blocks.len() - 20);
    }
    Ok(false)
}
