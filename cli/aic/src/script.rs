//! 脚本处理器（P11 / FR-SCR-001~003 / docs/01 §4.9；docs/04 §P11）。
//!
//! 设计：
//! - 实现为 `aic` 库内模块（单实现双入口）：`aic script run <file> [args...]`
//!   与独立 bin `aiscript`（调度器/CI 专用薄壳）共享同一引擎；
//!   另支持 FR-CLI-002 的 `aic @script` 位置参数形态。
//! - 纯文本脚本按行解析：`#` 注释、空行忽略、行尾 `&` 续行、双引号参数、
//!   命令大小写不敏感；变量替换 `%1..%9`（脚本位置参数，跳过 `-`/`/` 开关）、
//!   `%VAR%`（环境变量）、`%date%`/`%time%`/`%fn_time%`、`%%` 字面。
//! - 复用引擎能力：加载经 ai-dir 全量扫描（`LOAD`，失败终止脚本）；
//!   选择经 SELECT 掩码求值（EXPAND 控制可见性，FR-SCR-001 不可见会话模型）；
//!   操作经 ai-dir execute 层（COPY/MOVE/DELETE/RENAME/TOUCH 部分成功语义）；
//!   同步经 ai-dir run_sync；报告经 ai-core report 渲染器。
//! - 退出码（FR-CLI-004 脚本侧）：105 读脚本失败 / 106 语法错误 /
//!   107 加载文件夹或文件失败；运行期命令失败（stop-on-error 开启时中止）
//!   以 aic 约定退出码 2（文档化扩展，BC 的 105~107 仅覆盖加载类失败）。
//! - 不可见会话模型：EXPAND/COLLAPSE 维护展开集；SELECT 仅作用于可见行；
//!   FILTER 三类过滤作用于可见行；报告输出可见行。

use std::collections::{BTreeSet, HashMap};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::SystemTime;

use ai_core::{
    DirDisplayFilter, DirSnapshot, DisplayFilter, PatchStyle, ReportLayout, ReportNaming,
    ReportOptions, ReportSnapshot, TableSnapshot, TextSnapshot,
};
use ai_dir::{
    execute_ops, run_sync, CancelFlag, DirNode, DirScanOptions, ExecuteOptions, FileOp, NameFilter,
    OtherFilter, SyncRequest, SyncStrategy,
};
use ai_protocol::{
    CompareCriteria, ContentCompareMethod, ContentCompareMode, DirEntryStatus, EntryKind,
    OtherFilterSpec, SyncPreset,
};
use ai_vfs::{LocalFs, VfsPath, VfsProvider as _};

use crate::{EXIT_ERROR, EXIT_SAME};

/// FR-CLI-004：脚本文件读取失败
pub const EXIT_SCRIPT_READ: u8 = 105;
/// FR-CLI-004：脚本语法错误
pub const EXIT_SCRIPT_SYNTAX: u8 = 106;
/// FR-CLI-004：脚本无法加载文件夹或文件
pub const EXIT_SCRIPT_LOAD: u8 = 107;

/// 脚本执行选项
#[derive(Debug, Clone, Default)]
pub struct ScriptOptions {
    /// /silent：抑制状态输出（进度行）
    pub silent: bool,
    /// 变量注入（优先于环境变量；测试与调用方覆盖 %VAR% 用）
    pub vars: Vec<(String, String)>,
    /// 工作目录（相对路径解析基准；默认进程 cwd）
    pub cwd: Option<PathBuf>,
}

/// 脚本执行结果
#[derive(Debug, Clone, Default)]
pub struct ScriptOutcome {
    pub exit_code: u8,
    /// 状态行（stdout 语义；silent 时仍收集，便于测试断言）
    pub status: Vec<String>,
    /// 错误行
    pub errors: Vec<String>,
    /// LOG 产出（normal/verbose 级别时写文件；此处保留供测试断言）
    pub log: Vec<String>,
}

// ---------------------------------------------------------------------------
// 词法层：逻辑行 / 注释 / 分词 / 变量替换
// ---------------------------------------------------------------------------

/// 续行合并后的逻辑行（no = 首物理行号，1 起）
#[derive(Debug, Clone, PartialEq, Eq)]
struct LogicalLine {
    no: usize,
    text: String,
}

/// 去掉引号外的注释（`#` 起）
fn strip_comment(line: &str) -> &str {
    let mut in_quotes = false;
    for (i, c) in line.char_indices() {
        match c {
            '"' => in_quotes = !in_quotes,
            '#' if !in_quotes => return &line[..i],
            _ => {}
        }
    }
    line
}

/// 物理行 -> 逻辑行（`&` 续行合并；注释剥离；空行跳过）
fn logical_lines(src: &str) -> Vec<LogicalLine> {
    let mut out: Vec<LogicalLine> = Vec::new();
    let mut cur: Option<LogicalLine> = None;
    for (i, raw) in src.lines().enumerate() {
        let line = strip_comment(raw).trim().to_string();
        let continued = line.ends_with('&');
        let piece = line.trim_end_matches('&').trim().to_string();
        match &mut cur {
            Some(c) => {
                if !piece.is_empty() {
                    c.text.push(' ');
                    c.text.push_str(&piece);
                }
            }
            None => {
                cur = Some(LogicalLine {
                    no: i + 1,
                    text: piece,
                });
            }
        }
        if !continued {
            if let Some(c) = cur.take() {
                if !c.text.is_empty() {
                    out.push(c);
                }
            }
        }
    }
    if let Some(c) = cur.take() {
        if !c.text.is_empty() {
            out.push(c);
        }
    }
    out
}

/// 分词：空白分隔，双引号聚合为单词（引号剥离，内部空格保留）
fn tokenize(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut has_quote = false;
    for c in line.chars() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
                has_quote = true;
            }
            c if c.is_whitespace() && !in_quotes => {
                if !cur.is_empty() || has_quote {
                    out.push(std::mem::take(&mut cur));
                    has_quote = false;
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() || has_quote {
        out.push(cur);
    }
    out
}

fn now_parts() -> (i64, u32, u32, u32, u32, u32) {
    let ms = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let (y, mo, d, h, mi, s) = ai_core::civil_from_epoch_ms(ms);
    (y, mo, d, h, mi, s)
}

/// 变量替换：`%1..%9` 位置参数（跳过 `-`/`/` 开关的规则由调用方预处理 args）、
/// `%VAR%` 环境变量（extra 覆盖）、`%date%`（yyyy-mm-dd）、`%time%`
/// （hh:mm:ss 区域相关近似）、`%fn_time%`（hh-mm-ss 文件名安全）、`%%` 字面。
pub fn substitute_vars(line: &str, args: &[String], extra: &HashMap<String, String>) -> String {
    let (y, mo, d, h, mi, s) = now_parts();
    let dynamic: [(&str, String); 3] = [
        ("date", format!("{y:04}-{mo:02}-{d:02}")),
        ("time", format!("{h:02}:{mi:02}:{s:02}")),
        ("fn_time", format!("{h:02}-{mi:02}-{s:02}")),
    ];
    let mut out = String::with_capacity(line.len());
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        if c != '%' {
            out.push(c);
            i += 1;
            continue;
        }
        // %1..%9
        if let Some(digit) = chars.get(i + 1).and_then(|c| c.to_digit(10)) {
            if (1..=9).contains(&digit) {
                if let Some(a) = args.get(digit as usize - 1) {
                    out.push_str(a);
                }
                i += 2;
                continue;
            }
        }
        if chars.get(i + 1) == Some(&'%') {
            out.push('%');
            i += 2;
            continue;
        }
        // %date% / %time% / %fn_time% / %VAR%
        if chars[i + 1..].contains(&'%') {
            let probe: String = chars[i + 1..].iter().collect();
            if let Some(name) = probe.split('%').next() {
                let known = dynamic.iter().find(|(k, _)| *k == name);
                if let Some((_, v)) = known {
                    out.push_str(v);
                    i += name.len() + 2;
                    continue;
                }
                // %VAR% 环境变量 / 注入变量（大小写敏感）
                if !name.is_empty() {
                    if let Some(v) = extra.get(name) {
                        out.push_str(v);
                    } else if let Ok(v) = std::env::var(name) {
                        out.push_str(&v);
                    }
                    i += name.len() + 2;
                    continue;
                }
            }
        }
        out.push('%');
        i += 1;
    }
    out
}

/// 脚本位置参数：FR-SCR-001「脚本名与开关不计入」——调用方已剥离脚本名；
/// 开关剥离：Windows 为 `/` 前缀，macOS/Linux 为 `-` 前缀（FR-CLI-003 平台约定，
/// Unix 绝对路径以 `/` 开头须保留为位置参数）。
pub fn script_positional_args(tokens: &[String]) -> Vec<String> {
    let is_switch = |t: &str| -> bool {
        #[cfg(windows)]
        {
            t.starts_with('/') || t.starts_with('-')
        }
        #[cfg(not(windows))]
        {
            let _ = t;
            t.starts_with('-')
        }
    };
    tokens.iter().filter(|t| !is_switch(t)).cloned().collect()
}

// ---------------------------------------------------------------------------
// 命令 AST 与解析
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SideWord {
    LeftToRight,
    RightToLeft,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SideSel {
    Left,
    Right,
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompareWord {
    Crc,
    Binary,
    RulesBased,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LogWord {
    None,
    Normal,
    Verbose,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Cmd {
    Load {
        left: String,
        right: Option<String>,
        create: Option<String>,
    },
    Select {
        masks: Vec<String>,
    },
    Filter {
        name: Option<String>,
        cutoff: Option<String>,
        size: Option<String>,
        attrib: Option<String>,
        ignored: Vec<String>,
    },
    Expand {
        all: bool,
        paths: Vec<String>,
    },
    Collapse {
        all: bool,
        paths: Vec<String>,
    },
    Compare {
        mode: Option<CompareWord>,
    },
    Criteria {
        tokens: Vec<String>,
    },
    Copy {
        dir: SideWord,
    },
    Move {
        dir: SideWord,
    },
    CopyTo {
        side: SideSel,
        path: String,
    },
    MoveTo {
        side: SideSel,
        path: String,
    },
    Delete {
        side: SideSel,
        recyclebin: Option<String>,
    },
    Mkdir {
        side: SideSel,
        path: String,
    },
    Rename {
        regexpr: Option<String>,
        pattern: String,
    },
    Touch {
        copy: Option<SideWord>,
        set: Option<(SideSel, String)>,
    },
    Attrib {
        set: bool,
        letters: String,
        paths: Vec<String>,
    },
    Sync {
        kind: SideWord,
        update: bool,
        all: bool,
        visible: bool,
        create_empty: bool,
    },
    Snapshot {
        side: SideSel,
        path: Option<String>,
        no_filters: bool,
        output: String,
    },
    FolderReport {
        spec: ReportSpec,
    },
    TextReport {
        spec: ReportSpec,
    },
    HexReport {
        spec: ReportSpec,
    },
    TableReport {
        spec: ReportSpec,
    },
    FileReport {
        spec: ReportSpec,
    },
    ViewReport {
        kind: &'static str,
        spec: ReportSpec,
    },
    Log {
        level: LogWord,
        file: Option<(bool, String)>,
    },
    Option {
        stop_on_error: Option<bool>,
        confirm: Option<String>,
    },
    Beep,
    IfExists {
        path: String,
        negate: bool,
        rest: Vec<String>,
    },
}

/// 报告命令公共参数（FR-SCR-003）
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ReportSpec {
    layout: Option<ReportLayout>,
    title: Option<String>,
    display: Option<DisplayFilter>,
    dir_display: Option<DirDisplayFilter>,
    ignore_unimportant: bool,
    line_numbers: bool,
    context: Option<u32>,
    patch_style: Option<PatchStyle>,
    tsv: bool,
    /// output-to: printer | clipboard | <文件>
    output_to: Option<String>,
    /// 显式忽略的选项 token（output-options: 之后的自由参数）
    ignored: Vec<String>,
}

fn parse_err(no: usize, msg: impl Into<String>) -> String {
    format!("第 {no} 行: {}", msg.into())
}

/// `key:value` 拆分（保留其余内容；key 大小写不敏感）
fn split_kv(tok: &str) -> Option<(String, String)> {
    let (k, v) = tok.split_once(':')?;
    Some((k.to_ascii_lowercase(), v.to_string()))
}

fn strip_parens(v: &str) -> &str {
    let t = v.trim();
    if t.starts_with('(') && t.ends_with(')') && t.len() >= 2 {
        &t[1..t.len() - 1]
    } else {
        t
    }
}

fn parse_report_spec(no: usize, verb: &str, tokens: &[String]) -> Result<ReportSpec, String> {
    let mut spec = ReportSpec::default();
    let mut i = 0usize;
    let mut collecting_output_opts = false;
    while i < tokens.len() {
        let tok = &tokens[i];
        match split_kv(tok) {
            Some((k, v)) if k == "layout" => {
                spec.layout = ReportLayout::parse(strip_parens(&v));
                if spec.layout.is_none() {
                    return Err(parse_err(no, format!("{verb} layout 值不合法: {v}")));
                }
                collecting_output_opts = false;
            }
            Some((k, v)) if k == "title" => {
                spec.title = Some(strip_parens(&v).to_string());
                collecting_output_opts = false;
            }
            Some((k, v)) if k == "output-to" => {
                spec.output_to = Some(strip_parens(&v).to_string());
                collecting_output_opts = false;
            }
            Some((k, v)) if k == "options" || k == "output-options" => {
                // 选项值后的自由 token 归入 ignored（wrap/print 样式等渲染层不消费）
                spec.ignored.push(v);
                collecting_output_opts = k == "output-options";
            }
            Some((k, v)) if k == "display" => {
                spec.display = match strip_parens(&v) {
                    "all" => Some(DisplayFilter::All),
                    "mismatches" => Some(DisplayFilter::Mismatches),
                    "context" => Some(DisplayFilter::Context),
                    "matches" => Some(DisplayFilter::Matches),
                    _ => None,
                };
                collecting_output_opts = false;
            }
            Some((k, v)) if k == "dir-display" || k == "display-filter" => {
                spec.dir_display = DirDisplayFilter::parse(strip_parens(&v));
                collecting_output_opts = false;
            }
            Some((k, v)) if k == "patch-style" || k == "patch" => {
                spec.patch_style = match strip_parens(&v) {
                    "unified" => Some(PatchStyle::Unified),
                    "context" => Some(PatchStyle::Context),
                    "normal" => Some(PatchStyle::Normal),
                    _ => None,
                };
                collecting_output_opts = false;
            }
            Some((k, v)) if k == "context" || k == "context-lines" => {
                spec.context = strip_parens(&v).parse().ok();
                collecting_output_opts = false;
            }
            Some((k, _)) if k == "ignore-unimportant" || k == "iu" => {
                spec.ignore_unimportant = true;
                collecting_output_opts = false;
            }
            Some((k, _)) if k == "line-numbers" || k == "ln" => {
                spec.line_numbers = true;
                collecting_output_opts = false;
            }
            Some((k, _)) if k == "tsv" => {
                spec.tsv = true;
                collecting_output_opts = false;
            }
            _ => {
                if collecting_output_opts {
                    spec.ignored.push(tok.clone());
                } else if !tok.contains(':') && !tok.starts_with('-') {
                    // output-to 允许裸路径（无 output-to: 前缀的最后一个裸 token）
                    spec.output_to = Some(tok.clone());
                } else if let Some((k, _)) = split_kv(tok) {
                    // Windows 盘符路径（"C:\out\r.html"）：单字符键 = 盘符，不吞
                    if k.len() == 1 && k.chars().all(|c| c.is_ascii_alphabetic()) {
                        spec.output_to = Some(tok.clone());
                    } else {
                        spec.ignored.push(tok.clone());
                    }
                } else {
                    spec.ignored.push(tok.clone());
                }
            }
        }
        i += 1;
    }
    Ok(spec)
}

fn parse_side_word(v: &str) -> Option<SideWord> {
    match v.to_ascii_lowercase().as_str() {
        "left->right" => Some(SideWord::LeftToRight),
        "right->left" => Some(SideWord::RightToLeft),
        _ => None,
    }
}

fn parse_side_sel(v: &str) -> Option<SideSel> {
    match v.to_ascii_lowercase().as_str() {
        "left" => Some(SideSel::Left),
        "right" => Some(SideSel::Right),
        "all" => Some(SideSel::All),
        _ => None,
    }
}

/// 逻辑行 -> 命令（大小写不敏感；未知命令 = 语法错误 106）
fn parse_command(line: &LogicalLine) -> Result<Cmd, String> {
    let tokens = tokenize(&line.text);
    let Some(verb) = tokens.first() else {
        return Err(parse_err(line.no, "空命令"));
    };
    let v = verb.to_ascii_lowercase();
    let rest = &tokens[1..];
    let need = |cond: bool, msg: &str| -> Result<(), String> {
        if cond {
            Ok(())
        } else {
            Err(parse_err(line.no, msg))
        }
    };
    match v.as_str() {
        "load" | "loadcomparison" => {
            let mut create = None;
            let mut paths: Vec<String> = Vec::new();
            for tok in rest {
                match split_kv(tok) {
                    Some((k, val)) if k == "create" => create = Some(val),
                    _ => paths.push(tok.clone()),
                }
            }
            if paths.len() == 1 && paths[0].eq_ignore_ascii_case("default") {
                return Ok(Cmd::Load {
                    left: "default".into(),
                    right: None,
                    create,
                });
            }
            need(
                !paths.is_empty() && paths.len() <= 2,
                "load 需要 1~2 个路径参数（<左> [<右>]）",
            )?;
            Ok(Cmd::Load {
                left: paths[0].clone(),
                right: paths.get(1).cloned(),
                create,
            })
        }
        "savecomparison" => {
            // 别名：savecomparison <file> —— 快照保存别名（SNAPSHOT 子集）
            need(!rest.is_empty(), "savecomparison 需要 <目标文件> 参数")?;
            Ok(Cmd::Snapshot {
                side: SideSel::All,
                path: None,
                no_filters: false,
                output: rest.last().unwrap().clone(),
            })
        }
        "select" => {
            need(!rest.is_empty(), "select 需要至少一个掩码")?;
            Ok(Cmd::Select {
                masks: rest.to_vec(),
            })
        }
        "filter" => {
            let mut name = None;
            let mut cutoff = None;
            let mut size = None;
            let mut attrib = None;
            let mut ignored = Vec::new();
            for tok in rest {
                match split_kv(tok) {
                    Some((k, val)) if k == "cutoff" => cutoff = Some(val),
                    Some((k, val)) if k == "size" => size = Some(val),
                    Some((k, val)) if k == "attrib" => attrib = Some(val),
                    Some((k, _)) if k == "exclude-protected" || k == "include-protected" => {
                        ignored.push(tok.clone())
                    }
                    Some((k, val)) if k == "unixtype" => ignored.push(format!("unixtype:{val}")),
                    _ => name = Some(tok.clone()),
                }
            }
            Ok(Cmd::Filter {
                name,
                cutoff,
                size,
                attrib,
                ignored,
            })
        }
        "expand" => {
            let (all, paths) = all_or_paths(rest);
            need(all || !paths.is_empty(), "expand 需要 all 或路径")?;
            Ok(Cmd::Expand { all, paths })
        }
        "collapse" => {
            let (all, paths) = all_or_paths(rest);
            need(all || !paths.is_empty(), "collapse 需要 all 或路径")?;
            Ok(Cmd::Collapse { all, paths })
        }
        "compare" => {
            let mode =
                rest.first()
                    .map(|t| t.to_ascii_lowercase())
                    .and_then(|t| match t.as_str() {
                        "crc" => Some(CompareWord::Crc),
                        "binary" => Some(CompareWord::Binary),
                        "rules-based" | "rules" => Some(CompareWord::RulesBased),
                        _ => None,
                    });
            Ok(Cmd::Compare { mode })
        }
        "criteria" => Ok(Cmd::Criteria {
            tokens: rest.to_vec(),
        }),
        "copy" => {
            need(rest.len() == 1, "copy 需要 (left->right|right->left)")?;
            let dir =
                parse_side_word(&rest[0]).ok_or_else(|| parse_err(line.no, "copy 方向不合法"))?;
            Ok(Cmd::Copy { dir })
        }
        "move" => {
            need(rest.len() == 1, "move 需要 (left->right|right->left)")?;
            let dir =
                parse_side_word(&rest[0]).ok_or_else(|| parse_err(line.no, "move 方向不合法"))?;
            Ok(Cmd::Move { dir })
        }
        "copyto" | "moveto" => {
            let mut side = SideSel::All;
            let mut path: Option<String> = None;
            for tok in rest {
                match split_kv(tok) {
                    Some((k, val)) if k == "path" => path = Some(val),
                    Some((k, _))
                        if k == "path-relative" || k == "path-base" || k == "path-none" => {}
                    _ => {
                        if let Some(s) = parse_side_sel(tok) {
                            side = s;
                        } else {
                            path = Some(tok.clone());
                        }
                    }
                }
            }
            need(path.is_some(), "copyto/moveto 需要 <路径> 参数")?;
            let path = path.unwrap();
            let cmd = if v == "copyto" {
                Cmd::CopyTo { side, path }
            } else {
                Cmd::MoveTo { side, path }
            };
            Ok(cmd)
        }
        "delete" => {
            let mut side = None;
            let mut recyclebin = None;
            for tok in rest {
                match split_kv(tok) {
                    Some((k, val)) if k == "recyclebin" => recyclebin = Some(val),
                    _ => {
                        if side.is_none() {
                            side = parse_side_sel(tok);
                        }
                    }
                }
            }
            need(side.is_some(), "delete 需要 (left|right|all)")?;
            Ok(Cmd::Delete {
                side: side.unwrap(),
                recyclebin,
            })
        }
        "mkdir" => {
            let mut side = None;
            let mut path: Option<String> = None;
            for tok in rest {
                if let Some(s) = parse_side_sel(tok) {
                    side = Some(s);
                } else {
                    path = Some(tok.clone());
                }
            }
            need(
                side.is_some() && path.is_some(),
                "mkdir 需要 (left|right|all) <路径>",
            )?;
            Ok(Cmd::Mkdir {
                side: side.unwrap(),
                path: path.unwrap(),
            })
        }
        "rename" => {
            let mut regexpr = None;
            let mut pattern: Option<String> = None;
            let mut i = 0usize;
            while i < rest.len() {
                if rest[i].eq_ignore_ascii_case("regexpr") {
                    regexpr = rest.get(i + 1).cloned();
                    i += 2;
                } else {
                    pattern = Some(rest[i].clone());
                    i += 1;
                }
            }
            need(pattern.is_some(), "rename 需要 <新掩码>")?;
            Ok(Cmd::Rename {
                regexpr,
                pattern: pattern.unwrap(),
            })
        }
        "touch" => {
            // touch (left->right|right->left) | touch (left|right|all):(now|<时间戳>)
            if let Some(t0) = rest.first() {
                if let Some(d) = parse_side_word(t0) {
                    return Ok(Cmd::Touch {
                        copy: Some(d),
                        set: None,
                    });
                }
                if let Some((k, val)) = split_kv(t0) {
                    if let Some(_side) = parse_side_sel(&k) {
                        return Ok(Cmd::Touch {
                            copy: None,
                            set: Some((parse_side_sel(&k).unwrap(), val)),
                        });
                    }
                }
            }
            Err(parse_err(
                line.no,
                "touch 需要 (left->right|right->left) 或 (left|right|all):(now|时间戳)",
            ))
        }
        "attrib" => {
            let Some(t0) = rest.first() else {
                return Err(parse_err(line.no, "attrib 需要 (+|-)<属性集>"));
            };
            let (set, letters) = if let Some(l) = t0.strip_prefix('+') {
                (true, l.to_string())
            } else if let Some(l) = t0.strip_prefix('-') {
                (false, l.to_string())
            } else {
                return Err(parse_err(line.no, "attrib 属性集须以 +/- 开头"));
            };
            Ok(Cmd::Attrib {
                set,
                letters,
                paths: rest[1..].to_vec(),
            })
        }
        "sync" => {
            let mut update = true;
            let mut kind = SideWord::LeftToRight;
            let mut all = false;
            let mut visible = false;
            let mut create_empty = false;
            for tok in rest {
                match split_kv(tok) {
                    Some((k, val)) if k == "update" => {
                        update = true;
                        if let Some(d) = parse_side_word(&val) {
                            kind = d;
                        } else if val.eq_ignore_ascii_case("all") {
                            all = true;
                        } else {
                            return Err(parse_err(
                                line.no,
                                "sync 方向不合法（left->right|right->left|all）",
                            ));
                        }
                    }
                    Some((k, val)) if k == "mirror" => {
                        update = false;
                        match parse_side_word(&val) {
                            Some(d) => kind = d,
                            None => {
                                return Err(parse_err(
                                    line.no,
                                    "mirror 需要明确方向（left->right|right->left）",
                                ))
                            }
                        }
                    }
                    Some((k, _)) if k == "visible" => visible = true,
                    Some((k, _)) if k == "create-empty" => create_empty = true,
                    _ => {}
                }
            }
            Ok(Cmd::Sync {
                kind,
                update,
                all,
                visible,
                create_empty,
            })
        }
        "snapshot" => {
            let mut side = SideSel::Left;
            let mut path = None;
            let mut no_filters = false;
            let mut output = None;
            for tok in rest {
                match split_kv(tok) {
                    Some((k, _))
                        if k == "save-crc"
                            || k == "save-version"
                            || k == "expand-archives"
                            || k == "follow-symlinks"
                            || k == "include-empty" => {}
                    Some((k, ..)) if k == "no-filters" => no_filters = true,
                    Some((k, val)) if k == "output" => output = Some(val),
                    Some((k, val)) if k == "path" => path = Some(val),
                    _ => {
                        if let Some(s) = parse_side_sel(tok) {
                            side = s;
                        } else {
                            path = Some(tok.clone());
                        }
                    }
                }
            }
            let output = output.ok_or_else(|| parse_err(line.no, "snapshot 需要 output:<目标>"))?;
            Ok(Cmd::Snapshot {
                side,
                path,
                no_filters,
                output,
            })
        }
        "folder-report" => Ok(Cmd::FolderReport {
            spec: parse_report_spec(line.no, "folder-report", rest)?,
        }),
        "text-report" => Ok(Cmd::TextReport {
            spec: parse_report_spec(line.no, "text-report", rest)?,
        }),
        "hex-report" => Ok(Cmd::HexReport {
            spec: parse_report_spec(line.no, "hex-report", rest)?,
        }),
        "table-report" | "data-report" => Ok(Cmd::TableReport {
            spec: parse_report_spec(line.no, "table-report", rest)?,
        }),
        "file-report" => Ok(Cmd::FileReport {
            spec: parse_report_spec(line.no, "file-report", rest)?,
        }),
        "media-report" => Ok(Cmd::ViewReport {
            kind: "media",
            spec: parse_report_spec(line.no, "media-report", rest)?,
        }),
        "picture-report" => Ok(Cmd::ViewReport {
            kind: "picture",
            spec: parse_report_spec(line.no, "picture-report", rest)?,
        }),
        "registry-report" => Ok(Cmd::ViewReport {
            kind: "registry",
            spec: parse_report_spec(line.no, "registry-report", rest)?,
        }),
        "version-report" => Ok(Cmd::ViewReport {
            kind: "version",
            spec: parse_report_spec(line.no, "version-report", rest)?,
        }),
        "log" => {
            let mut level = LogWord::Normal;
            let mut file = None;
            for tok in rest {
                let t = tok.to_ascii_lowercase();
                match t.as_str() {
                    "none" => level = LogWord::None,
                    "normal" => level = LogWord::Normal,
                    "verbose" => level = LogWord::Verbose,
                    _ => match split_kv(tok) {
                        Some((k, val)) if k == "append" => file = Some((true, val)),
                        _ => file = Some((false, tok.clone())),
                    },
                }
            }
            Ok(Cmd::Log { level, file })
        }
        "option" => {
            let mut stop_on_error = None;
            let mut confirm = None;
            for tok in rest {
                let t = tok.to_ascii_lowercase();
                if let Some((k, val)) = split_kv(&t) {
                    if k == "confirm" {
                        confirm = Some(val);
                        continue;
                    }
                    if k == "stop-on-error" {
                        stop_on_error = Some(val != "no" && val != "false");
                        continue;
                    }
                }
                if t == "stop-on-error" {
                    stop_on_error = Some(true);
                }
            }
            Ok(Cmd::Option {
                stop_on_error,
                confirm,
            })
        }
        "beep" => Ok(Cmd::Beep),
        "if-exists" | "if-not-exists" => {
            need(!rest.is_empty(), "if-exists 需要 <路径> [THEN] <命令>")?;
            let path = rest[0].clone();
            let mut idx = 1usize;
            if rest
                .get(idx)
                .map(|t| t.eq_ignore_ascii_case("then"))
                .unwrap_or(false)
            {
                idx += 1;
            }
            Ok(Cmd::IfExists {
                path,
                negate: v == "if-not-exists",
                rest: rest[idx..].to_vec(),
            })
        }
        _ => Err(parse_err(line.no, format!("未知命令: {verb}"))),
    }
}

fn all_or_paths(rest: &[String]) -> (bool, Vec<String>) {
    let mut all = false;
    let mut paths = Vec::new();
    for t in rest {
        if t.eq_ignore_ascii_case("all") {
            all = true;
        } else {
            paths.push(t.clone());
        }
    }
    (all, paths)
}

// ---------------------------------------------------------------------------
// SELECT 掩码
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SelSide {
    All,
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SelCond {
    All,
    Exact,
    Diff,
    Newer,
    Older,
    Orphan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SelTy {
    All,
    Files,
    Folders,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SelectMask {
    side: SelSide,
    cond: SelCond,
    ty: SelTy,
    empty_folders: bool,
}

/// FR-SCR-002 SELECT 掩码：all | [(left|right|all).][(exact|diff|newer|older|orphan|all).]
/// [(files|folders|all)] | empty.folders（多掩码并集）
fn parse_select_mask(tok: &str) -> Option<SelectMask> {
    let t = tok.to_ascii_lowercase();
    if t == "all" {
        return Some(SelectMask {
            side: SelSide::All,
            cond: SelCond::All,
            ty: SelTy::All,
            empty_folders: false,
        });
    }
    if t == "empty.folders" {
        return Some(SelectMask {
            side: SelSide::All,
            cond: SelCond::All,
            ty: SelTy::Folders,
            empty_folders: true,
        });
    }
    let mut side = SelSide::All;
    let mut cond = SelCond::All;
    let mut ty = SelTy::All;
    let mut saw_any = false;
    for part in t.split('.') {
        match part {
            "left" => {
                side = SelSide::Left;
                saw_any = true;
            }
            "right" => {
                side = SelSide::Right;
                saw_any = true;
            }
            "all" => {
                saw_any = true;
            }
            "exact" | "same" => {
                cond = SelCond::Exact;
                saw_any = true;
            }
            "diff" => {
                cond = SelCond::Diff;
                saw_any = true;
            }
            "newer" => {
                cond = SelCond::Newer;
                saw_any = true;
            }
            "older" => {
                cond = SelCond::Older;
                saw_any = true;
            }
            "orphan" => {
                cond = SelCond::Orphan;
                saw_any = true;
            }
            "files" => {
                ty = SelTy::Files;
                saw_any = true;
            }
            "folders" => {
                ty = SelTy::Folders;
                saw_any = true;
            }
            _ => return None,
        }
    }
    saw_any.then_some(SelectMask {
        side,
        cond,
        ty,
        empty_folders: false,
    })
}

impl SelectMask {
    fn keeps(&self, row: &DirNode) -> bool {
        // 侧
        let side_ok = match self.side {
            SelSide::All => row.left.is_some() || row.right.is_some(),
            SelSide::Left => row.left.is_some(),
            SelSide::Right => row.right.is_some(),
        };
        if !side_ok {
            return false;
        }
        // 条件
        let cond_ok = match self.cond {
            SelCond::All => true,
            SelCond::Exact => matches!(row.status, DirEntryStatus::Same | DirEntryStatus::DirSame),
            SelCond::Diff => matches!(
                row.status,
                DirEntryStatus::QuickDiff | DirEntryStatus::ContentDiff | DirEntryStatus::DirDiff
            ),
            SelCond::Newer => match (&row.left, &row.right) {
                (Some(l), Some(r)) => l.mtime_ms > r.mtime_ms,
                _ => false,
            },
            SelCond::Older => match (&row.left, &row.right) {
                (Some(l), Some(r)) => l.mtime_ms < r.mtime_ms,
                _ => false,
            },
            SelCond::Orphan => match self.side {
                SelSide::Left => row.status == DirEntryStatus::LeftOnly,
                SelSide::Right => row.status == DirEntryStatus::RightOnly,
                SelSide::All => matches!(
                    row.status,
                    DirEntryStatus::LeftOnly | DirEntryStatus::RightOnly
                ),
            },
        };
        if !cond_ok {
            return false;
        }
        // 类型
        match self.ty {
            SelTy::All => true,
            SelTy::Files => row.kind == EntryKind::File,
            SelTy::Folders => row.kind == EntryKind::Dir,
        }
    }
}

// ---------------------------------------------------------------------------
// 运行状态
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfirmMode {
    Prompt,
    YesToAll,
    NoToAll,
}

enum LoadedSession {
    /// 文件夹比较（全量扫描 arena；EXPAND 控制可见性）
    Dir {
        left: VfsPath,
        right: VfsPath,
        left_s: String,
        right_s: String,
        arena: Vec<DirNode>,
        stats: ai_protocol::DirStats,
    },
    /// 文件对（text/table/hex 报告数据源）
    Files {
        left: VfsPath,
        right: VfsPath,
        left_s: String,
        right_s: String,
    },
}

struct ScriptState {
    vfs: LocalFs,
    session: Option<LoadedSession>,
    select_masks: Vec<SelectMask>,
    name_filter: Option<NameFilter>,
    other_filter: OtherFilter,
    criteria: CompareCriteria,
    /// 基于规则内容比较的重要性模型（CRITERIA ignore-unimportant 设置）
    importance: ai_core::ImportanceOptions,
    stop_on_error: bool,
    confirm: ConfirmMode,
    log_level: LogWord,
    log_file: Option<(bool, String)>,
    expanded: BTreeSet<String>,
}

impl ScriptState {
    fn new() -> Self {
        ScriptState {
            vfs: LocalFs::new(),
            session: None,
            select_masks: Vec::new(),
            name_filter: None,
            other_filter: OtherFilter::from_spec(OtherFilterSpec::default()),
            // 脚本默认判据 = 内容比较（BC 脚本语义：规则/内容判等，不做 mtime 快速判同）
            criteria: CompareCriteria {
                mtime_tolerance_sec: 0,
                content_compare: ContentCompareMode::Always,
                ..CompareCriteria::default()
            },
            importance: ai_core::ImportanceOptions::default(),
            stop_on_error: false,
            confirm: ConfirmMode::Prompt,
            log_level: LogWord::None,
            log_file: None,
            expanded: BTreeSet::from([String::new()]), // 根默认展开
        }
    }

    /// 行可见性：全部祖先目录已展开（FR-SCR-001 EXPAND 模型）
    fn row_visible(&self, row: &DirNode) -> bool {
        let mut dir = row.rel_path.rsplit_once('/').map(|(p, _)| p.to_string());
        loop {
            match dir {
                Some(d) => {
                    if !self.expanded.contains(&d) {
                        return false;
                    }
                    dir = d.rsplit_once('/').map(|(p, _)| p.to_string());
                }
                None => return true, // 顶层（父 = 根，恒展开）
            }
        }
    }

    /// 可见 + 名称过滤 + 其他过滤
    fn row_shown(&self, row: &DirNode) -> bool {
        if !self.row_visible(row) {
            return false;
        }
        if let Some(f) = &self.name_filter {
            let segs: Vec<&str> = row.rel_path.split('/').filter(|s| !s.is_empty()).collect();
            let ok = match row.kind {
                EntryKind::Dir => f.dir_visible(&segs),
                _ => f.file_allowed(&segs),
            };
            if !ok {
                return false;
            }
        }
        let info = row.left.as_ref().or(row.right.as_ref());
        if let Some(i) = info {
            let s = self.other_filter.spec();
            if let Some(min) = s.min_size {
                if i.size < min {
                    return false;
                }
            }
            if let Some(max) = s.max_size {
                if i.size > max {
                    return false;
                }
            }
            if let Some(min) = s.min_mtime_ms {
                if i.mtime_ms < min {
                    return false;
                }
            }
            if let Some(max) = s.max_mtime_ms {
                if i.mtime_ms > max {
                    return false;
                }
            }
            if let Some(inc) = &s.attrs_include {
                let attr = i.attribute.clone().unwrap_or_default();
                if !inc.chars().all(|c| attr.contains(c)) {
                    return false;
                }
            }
            if let Some(exc) = &s.attrs_exclude {
                let attr = i.attribute.clone().unwrap_or_default();
                if exc.chars().any(|c| attr.contains(c)) {
                    return false;
                }
            }
        }
        true
    }

    fn dir_parts(&self) -> Option<(VfsPath, VfsPath, String, String)> {
        match &self.session {
            Some(LoadedSession::Dir {
                left,
                right,
                left_s,
                right_s,
                ..
            }) => Some((left.clone(), right.clone(), left_s.clone(), right_s.clone())),
            _ => None,
        }
    }

    fn files_parts(&self) -> Option<(VfsPath, VfsPath, String, String)> {
        match &self.session {
            Some(LoadedSession::Files {
                left,
                right,
                left_s,
                right_s,
            }) => Some((left.clone(), right.clone(), left_s.clone(), right_s.clone())),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// 执行器
// ---------------------------------------------------------------------------

/// 脚本错误（加载类错误终止脚本并带 105/106/107 退出码）
enum ScriptError {
    Syntax(String),
    LoadFail(String),
    Runtime(String),
}

/// 执行脚本文本（变量注入由 opts.vars 提供；返回退出码与日志）
pub fn run_script(src: &str, args: &[String], opts: &ScriptOptions) -> ScriptOutcome {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio 运行时");
    rt.block_on(run_script_async(src, args, opts))
}

pub async fn run_script_async(src: &str, args: &[String], opts: &ScriptOptions) -> ScriptOutcome {
    let mut outcome = ScriptOutcome::default();
    let mut extra: HashMap<String, String> = opts.vars.iter().cloned().collect();
    for (k, v) in std::env::vars() {
        extra.entry(k).or_insert(v);
    }
    let lines = logical_lines(src);
    let mut state = ScriptState::new();
    let total = lines.len();

    for (idx, line) in lines.iter().enumerate() {
        let substituted = substitute_vars(&line.text, args, &extra);
        let tokens = tokenize(&substituted);
        let cmdline = tokens.join(" ");
        let status_line = format!("[{}/{}] {}", idx + 1, total, cmdline);
        outcome.status.push(status_line.clone());
        if !opts.silent {
            eprintln!("{status_line}");
        }

        let cmd = match parse_command(&LogicalLine {
            no: line.no,
            text: substituted.clone(),
        }) {
            Ok(c) => c,
            Err(e) => {
                outcome.errors.push(format!("语法错误 {e}"));
                outcome.exit_code = EXIT_SCRIPT_SYNTAX;
                return outcome;
            }
        };

        match exec_cmd(&mut state, cmd, opts, &mut outcome).await {
            Ok(()) => {}
            Err(ScriptError::Syntax(m)) => {
                outcome.errors.push(m);
                // FR-CLI-004：106 脚本语法错误（立即终止）
                outcome.exit_code = EXIT_SCRIPT_SYNTAX;
                return outcome;
            }
            Err(ScriptError::LoadFail(m)) => {
                outcome.errors.push(m);
                // FR-CLI-004：107 脚本无法加载文件夹或文件（立即终止）
                outcome.exit_code = EXIT_SCRIPT_LOAD;
                return outcome;
            }
            Err(ScriptError::Runtime(m)) => {
                outcome.errors.push(m);
                if state.stop_on_error {
                    outcome.exit_code = EXIT_ERROR;
                    return outcome;
                }
            }
        }
    }

    // 写日志（LOG normal/verbose）
    if state.log_level != LogWord::None {
        let path = state
            .log_file
            .as_ref()
            .map(|(_, p)| p.clone())
            .unwrap_or_else(|| "Log.txt".to_string());
        let mut body = String::new();
        for l in &outcome.status {
            body.push_str(l);
            body.push('\n');
        }
        if state.log_level == LogWord::Verbose {
            for e in &outcome.errors {
                body.push_str("ERROR ");
                body.push_str(e);
                body.push('\n');
            }
        }
        if let Some(err) = write_log(
            &path,
            state.log_file.as_ref().map(|(a, _)| *a).unwrap_or(false),
            &body,
        ) {
            outcome.errors.push(format!("日志写入失败 {path}: {err}"));
            if outcome.exit_code == EXIT_SAME {
                outcome.exit_code = EXIT_ERROR;
            }
        }
    }
    outcome.log = outcome.status.clone();
    outcome
}

fn write_log(path: &str, append: bool, body: &str) -> Option<String> {
    use std::io::Write as _;
    let opened = if append {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
    } else {
        std::fs::File::create(path)
    };
    let mut f = match opened {
        Ok(f) => f,
        Err(e) => return Some(format!("打开失败: {e}")),
    };
    if let Err(e) = f.write_all(body.as_bytes()) {
        return Some(format!("写入失败: {e}"));
    }
    None
}

/// exec_cmd 的错误码归类：Read/Syntax/LoadFail 立即终止（105/106/107）
async fn exec_cmd(
    state: &mut ScriptState,
    cmd: Cmd,
    opts: &ScriptOptions,
    outcome: &mut ScriptOutcome,
) -> Result<(), ScriptError> {
    match cmd {
        Cmd::Load {
            left,
            right,
            create,
        } => exec_load(state, &left, right.as_deref(), create.as_deref()).await,
        Cmd::Select { masks } => {
            let mut parsed = Vec::new();
            for m in &masks {
                match parse_select_mask(m) {
                    Some(x) => parsed.push(x),
                    None => return Err(ScriptError::Syntax(format!("select 掩码不合法: {m}"))),
                }
            }
            state.select_masks = parsed;
            Ok(())
        }
        Cmd::Filter {
            name,
            cutoff,
            size,
            attrib,
            ignored,
        } => exec_filter(state, name, cutoff, size, attrib, ignored),
        Cmd::Expand { all, paths } => {
            exec_expand(state, all, &paths, true);
            Ok(())
        }
        Cmd::Collapse { all, paths } => {
            exec_expand(state, all, &paths, false);
            Ok(())
        }
        Cmd::Compare { mode } => exec_compare(state, mode, outcome).await,
        Cmd::Criteria { tokens } => {
            exec_criteria(state, &tokens)?;
            Ok(())
        }
        Cmd::Copy { dir } => exec_cross_ops(state, dir, false, outcome).await,
        Cmd::Move { dir } => exec_cross_ops(state, dir, true, outcome).await,
        Cmd::CopyTo { side, path } => exec_to_path(state, side, &path, false, outcome).await,
        Cmd::MoveTo { side, path } => exec_to_path(state, side, &path, true, outcome).await,
        Cmd::Delete { side, recyclebin } => {
            if let Some(rb) = recyclebin {
                if rb.eq_ignore_ascii_case("yes") {
                    outcome.errors.push(
                        "提示: recyclebin=yes 在当前平台近似为直接删除（回收站未接入）".into(),
                    );
                }
            }
            exec_side_delete(state, side, outcome).await
        }
        Cmd::Mkdir { side, path } => exec_mkdir(state, side, &path, outcome).await,
        Cmd::Rename { regexpr, pattern } => exec_rename(state, regexpr, &pattern, outcome).await,
        Cmd::Touch { copy, set } => exec_touch(state, copy, set, outcome).await,
        Cmd::Attrib {
            set,
            letters,
            paths,
        } => exec_attrib(state, set, &letters, &paths, outcome).await,
        Cmd::Sync {
            kind,
            update,
            all,
            visible,
            create_empty,
        } => exec_sync(state, kind, update, all, visible, create_empty, outcome).await,
        Cmd::Snapshot {
            side,
            path,
            no_filters,
            output,
        } => exec_snapshot(state, side, path, no_filters, &output, opts, outcome),
        Cmd::FolderReport { spec } => exec_folder_report(state, spec, opts, outcome),
        Cmd::TextReport { spec } => exec_text_report(state, spec, opts, outcome).await,
        Cmd::HexReport { spec } => exec_hex_report(state, spec, opts, outcome).await,
        Cmd::TableReport { spec } => exec_table_report(state, spec, opts, outcome).await,
        Cmd::FileReport { spec } => exec_file_report(state, spec, opts, outcome).await,
        Cmd::ViewReport { kind, .. } => Err(ScriptError::Runtime(format!(
            "{kind}-REPORT 不可用: {kind} 比较引擎未实现（P14 排期）"
        ))),
        Cmd::Log { level, file } => {
            state.log_level = level;
            state.log_file = file;
            Ok(())
        }
        Cmd::Option {
            stop_on_error,
            confirm,
        } => {
            if let Some(s) = stop_on_error {
                state.stop_on_error = s;
            }
            if let Some(c) = confirm {
                state.confirm = match c.as_str() {
                    "yes-to-all" => ConfirmMode::YesToAll,
                    "no-to-all" => ConfirmMode::NoToAll,
                    _ => ConfirmMode::Prompt,
                };
            }
            Ok(())
        }
        Cmd::Beep => {
            // FR-SCR-002：BEEP 无视 /silent
            let _ = std::io::stderr().write_all(b"\x07");
            Ok(())
        }
        Cmd::IfExists { path, negate, rest } => {
            let p = Path::new(&path);
            let exists = p.exists();
            let go = if negate { !exists } else { exists };
            if !go {
                return Ok(());
            }
            if rest.is_empty() {
                return Ok(());
            }
            let sub = tokenize(&rest.join(" "));
            let sub_line = LogicalLine {
                no: 0,
                text: sub.join(" "),
            };
            let sub_cmd = parse_command(&sub_line).map_err(ScriptError::Syntax)?;
            Box::pin(exec_cmd(state, sub_cmd, opts, outcome)).await
        }
    }
}

// ---------------------------------------------------------------------------
// LOAD / EXPAND / FILTER / CRITERIA / COMPARE
// ---------------------------------------------------------------------------

async fn exec_load(
    state: &mut ScriptState,
    left: &str,
    right: Option<&str>,
    _create: Option<&str>,
) -> Result<(), ScriptError> {
    // load <default>：重置会话（默认新比较）
    if right.is_none() && left.eq_ignore_ascii_case("default") {
        state.session = None;
        state.select_masks.clear();
        state.expanded = BTreeSet::from([String::new()]);
        return Ok(());
    }
    let Some(right_s) = right else {
        return Err(ScriptError::LoadFail(
            "load 需要两侧路径（命名会话/工作区暂未支持，P17 排期）".into(),
        ));
    };
    let lp = VfsPath::parse(left)
        .map_err(|e| ScriptError::LoadFail(format!("路径不合法 {left}: {e}")))?;
    let rp = VfsPath::parse(right_s)
        .map_err(|e| ScriptError::LoadFail(format!("路径不合法 {right_s}: {e}")))?;
    let ls = state
        .vfs
        .stat(&lp)
        .await
        .map_err(|e| ScriptError::LoadFail(format!("无法加载 {left}: {e}")))?;
    let rs = state
        .vfs
        .stat(&rp)
        .await
        .map_err(|e| ScriptError::LoadFail(format!("无法加载 {right_s}: {e}")))?;

    state.select_masks.clear();
    state.expanded = BTreeSet::from([String::new()]);
    if ls.kind == ai_vfs::EntryKind::Dir && rs.kind == ai_vfs::EntryKind::Dir {
        let result = ai_dir::scan(
            &state.vfs,
            &lp,
            &rp,
            &DirScanOptions {
                depth: None,
                criteria: state.criteria.clone(),
                importance: state.importance.clone(),
            },
            &ai_dir::ByNameAligner,
            &CancelFlag::new(),
        )
        .await
        .map_err(|e| ScriptError::LoadFail(format!("文件夹扫描失败: {e}")))?;
        let stats = result.stats;
        state.session = Some(LoadedSession::Dir {
            left: lp,
            right: rp,
            left_s: left.to_string(),
            right_s: right_s.to_string(),
            arena: result.arena,
            stats,
        });
    } else if ls.kind == ai_vfs::EntryKind::File && rs.kind == ai_vfs::EntryKind::File {
        state.session = Some(LoadedSession::Files {
            left: lp,
            right: rp,
            left_s: left.to_string(),
            right_s: right_s.to_string(),
        });
    } else {
        return Err(ScriptError::LoadFail(format!(
            "load 两侧类型不一致（目录只能与目录比较）: {left} / {right_s}"
        )));
    }
    Ok(())
}

fn exec_expand(state: &mut ScriptState, all: bool, paths: &[String], expand: bool) {
    if all && expand {
        // 展开全部：标记哨兵 + 收集全部目录路径
        if let Some(LoadedSession::Dir { arena, .. }) = &state.session {
            for r in arena {
                if r.kind == EntryKind::Dir {
                    state.expanded.insert(r.rel_path.clone());
                }
            }
        }
        return;
    }
    if all && !expand {
        state.expanded = BTreeSet::from([String::new()]);
        return;
    }
    for p in paths {
        let p = p.trim_end_matches('/');
        if expand {
            state.expanded.insert(p.to_string());
        } else {
            // 折叠：移除该目录与其全部后代
            let prefix = format!("{p}/");
            state.expanded.retain(|d| d != p && !d.starts_with(&prefix));
        }
    }
}

fn exec_filter(
    state: &mut ScriptState,
    name: Option<String>,
    cutoff: Option<String>,
    size: Option<String>,
    attrib: Option<String>,
    ignored: Vec<String>,
) -> Result<(), ScriptError> {
    let _ = ignored; // exclude-protected / unixtype 等：接受并忽略（记录于命令回显）
    if let Some(mask) = name {
        let f = NameFilter::parse_shortcut(&mask, false);
        state.name_filter = Some(f);
    }
    if let Some(c) = cutoff {
        let mut spec = OtherFilterSpec::default();
        if !c.eq_ignore_ascii_case("none") {
            let ms = parse_cutoff_to_ms(&c)
                .ok_or_else(|| ScriptError::Syntax(format!("filter cutoff 值不合法: {c}")))?;
            spec.min_mtime_ms = Some(ms);
            state.other_filter = OtherFilter::from_spec(spec);
            return Ok(());
        }
        state.other_filter = OtherFilter::from_spec(spec);
    }
    if let Some(s) = size {
        let mut spec = OtherFilterSpec::default();
        if !s.eq_ignore_ascii_case("none") {
            let (op, n) = parse_size_filter(&s)
                .ok_or_else(|| ScriptError::Syntax(format!("filter size 值不合法: {s}")))?;
            match op {
                '<' => spec.max_size = Some(n),
                _ => spec.min_size = Some(n),
            }
        }
        state.other_filter = OtherFilter::from_spec(spec);
    }
    if let Some(a) = attrib {
        let mut spec = OtherFilterSpec::default();
        if !a.eq_ignore_ascii_case("none") {
            let (set, letters) = if let Some(l) = a.strip_prefix('+') {
                (true, l.to_string())
            } else if let Some(l) = a.strip_prefix('-') {
                (false, l.to_string())
            } else {
                return Err(ScriptError::Syntax(format!(
                    "filter attrib 需以 +/- 开头: {a}"
                )));
            };
            if set {
                spec.attrs_include = Some(letters);
            } else {
                spec.attrs_exclude = Some(letters);
            }
        }
        state.other_filter = OtherFilter::from_spec(spec);
    }
    Ok(())
}

/// cutoff:([<|>](<时间戳>|<N>days)|none) —— 宽松解析
fn parse_cutoff_to_ms(v: &str) -> Option<u64> {
    let v = v.trim();
    if let Some(stripped) = v.strip_suffix("days") {
        let n: u64 = stripped.trim().parse().ok()?;
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .ok()?
            .as_millis() as u64;
        return Some(now.saturating_sub(n * 86_400_000));
    }
    // 绝对时间戳：秒（10 位）或毫秒（13 位）
    let n: u64 = v.parse().ok()?;
    Some(if n < 100_000_000_000 { n * 1000 } else { n })
}

fn parse_size_filter(v: &str) -> Option<(char, u64)> {
    let v = v.trim();
    let (op, rest) = if let Some(r) = v.strip_prefix('<') {
        ('<', r)
    } else if let Some(r) = v.strip_prefix('>') {
        ('>', r)
    } else {
        ('>', v)
    };
    let rest = rest.trim();
    let mult = if let Some(n) = rest.strip_suffix("KB") {
        (1024.0, n)
    } else if let Some(n) = rest.strip_suffix("MB") {
        (1024.0 * 1024.0, n)
    } else if let Some(n) = rest.strip_suffix("GB") {
        (1024.0 * 1024.0 * 1024.0, n)
    } else if let Some(n) = rest.strip_suffix("TB") {
        (1024.0 * 1024.0 * 1024.0 * 1024.0, n)
    } else {
        (1.0, rest)
    };
    let n: f64 = mult.1.trim().parse().ok()?;
    Some((op, (n * mult.0) as u64))
}

/// BC `IGNORE-UNIMPORTANT` / `/iu` 的重要性模型：空白（首/嵌/尾）+ 大小写不重要。
/// 文本报告与目录规则比较共用，避免两处各写一份开关集。
fn ignore_unimportant_model() -> ai_core::ImportanceOptions {
    ai_core::ImportanceOptions {
        leading_ws_unimportant: true,
        embedded_ws_unimportant: true,
        trailing_ws_unimportant: true,
        case_unimportant: true,
        ..ai_core::ImportanceOptions::default()
    }
}

fn exec_criteria(state: &mut ScriptState, tokens: &[String]) -> Result<(), ScriptError> {
    for tok in tokens {
        let t = tok.to_ascii_lowercase();
        if let Some((k, v)) = split_kv(tok) {
            match k.as_str() {
                "timestamp" => {
                    // timestamp:[Nsec][;IgnoreDST]
                    let num: String = v.chars().take_while(|c| c.is_ascii_digit()).collect();
                    if !num.is_empty() {
                        state.criteria.mtime_tolerance_sec = num.parse().unwrap_or(u32::MAX);
                    }
                    continue;
                }
                "timezone" | "follow-symlinks" | "owner" | "group" | "permissions" | "version"
                | "attrib" => {
                    continue; // 接受并忽略（对应判据引擎侧未开放）
                }
                _ => {}
            }
        }
        match t.as_str() {
            "size" => {
                state.criteria.mtime_tolerance_sec = u32::MAX;
                state.criteria.content_compare = ContentCompareMode::Never;
            }
            "crc" => {
                state.criteria.content_compare = ContentCompareMode::Always;
                state.criteria.content_method = ContentCompareMethod::Crc;
                state.criteria.mtime_tolerance_sec = 0;
            }
            "binary" => {
                state.criteria.content_compare = ContentCompareMode::Always;
                state.criteria.content_method = ContentCompareMethod::Binary;
                state.criteria.mtime_tolerance_sec = 0;
            }
            "rules-based" => {
                state.criteria.content_compare = ContentCompareMode::Always;
                state.criteria.content_method = ContentCompareMethod::RulesBased;
                state.criteria.mtime_tolerance_sec = 0;
            }
            "ignore-unimportant" => {
                state.importance = ignore_unimportant_model();
            }
            _ => return Err(ScriptError::Syntax(format!("criteria 参数不合法: {tok}"))),
        }
    }
    Ok(())
}

async fn exec_compare(
    state: &mut ScriptState,
    mode: Option<CompareWord>,
    _outcome: &mut ScriptOutcome,
) -> Result<(), ScriptError> {
    let Some(LoadedSession::Dir { arena, .. }) = &mut state.session else {
        return Err(ScriptError::Runtime("COMPARE 需要先 LOAD 文件夹对".into()));
    };
    let method = match mode.unwrap_or(CompareWord::Crc) {
        CompareWord::Crc => ContentCompareMethod::Crc,
        CompareWord::Binary => ContentCompareMethod::Binary,
        CompareWord::RulesBased => ContentCompareMethod::RulesBased,
    };
    let mut new_arena = std::mem::take(arena);
    let selected = selected_indices(state, &new_arena);
    for &i in &selected {
        let row = &new_arena[i];
        if row.kind != EntryKind::File {
            continue;
        }
        let (Some(l), Some(r)) = (&row.left, &row.right) else {
            continue; // 孤立项跳过
        };
        let lp = dir_left_path(state, &new_arena[i]);
        let rp = dir_right_path(state, &new_arena[i]);
        // 判定权威在 ai_dir（R3-16）：size 判异、三方式分派、四态结果都在那里，
        // 脚本不再自写一套"逐行全等"近似（它会把仅不重要差异误判为重要差异）
        let verdict = ai_dir::content_verdict_stats(
            &state.vfs,
            &lp,
            &rp,
            ai_dir::compare::FileStatSummary::new(l.size, l.mtime_ms),
            ai_dir::compare::FileStatSummary::new(r.size, r.mtime_ms),
            method,
            &state.importance,
            None,
        )
        .await;
        new_arena[i].status = match verdict {
            Ok(v) => ai_dir::verdict_to_status(v),
            // IO 失败不静默判同（R3-09 口径）：标记未证实
            Err(_) => DirEntryStatus::QuickDiff,
        };
    }
    // 目录聚合重算 + 统计重算
    ai_dir::aggregate_all(&mut new_arena);
    let new_stats = ai_dir::compute_stats(&new_arena);
    if let Some(LoadedSession::Dir { arena, stats, .. }) = &mut state.session {
        *arena = new_arena;
        *stats = new_stats;
    }
    Ok(())
}

fn selected_indices(state: &ScriptState, arena: &[DirNode]) -> Vec<usize> {
    arena
        .iter()
        .enumerate()
        .skip(1)
        .filter(|(_, r)| state.row_shown(r))
        .filter(|(_, r)| {
            if state.select_masks.is_empty() {
                return true;
            }
            state.select_masks.iter().any(|m| m.keeps(r))
        })
        .map(|(i, _)| i)
        .collect()
}

fn dir_roots(state: &ScriptState) -> (VfsPath, VfsPath) {
    let (lp, rp, ..) = state.dir_parts().expect("目录会话");
    (lp, rp)
}

/// 操作落盘后重扫目录会话（arena/stats 刷新；FR-SCR 不可见会话模型与
/// 磁盘实态一致化——后续 SELECT/报告读到操作后的新状态）
async fn rescan_dir(state: &mut ScriptState) {
    let Some((lp, rp, ..)) = state.dir_parts() else {
        return;
    };
    let Ok(result) = ai_dir::scan(
        &state.vfs,
        &lp,
        &rp,
        &DirScanOptions {
            depth: None,
            criteria: state.criteria.clone(),
            importance: state.importance.clone(),
        },
        &ai_dir::ByNameAligner,
        &CancelFlag::new(),
    )
    .await
    else {
        return;
    };
    // 仅本次操作新出现的目录自动展开；已存在的目录保留用户 EXPAND/COLLAPSE 状态。
    // 全量展开会把后续 SELECT/DELETE 的作用域从顶层静默扩到整棵子树（数据丢失）。
    let old_dirs: std::collections::BTreeSet<String> = match &state.session {
        Some(LoadedSession::Dir { arena, .. }) => arena
            .iter()
            .filter(|r| r.kind == EntryKind::Dir)
            .map(|r| r.rel_path.clone())
            .collect(),
        _ => std::collections::BTreeSet::new(),
    };
    for r in result.arena.iter().filter(|r| r.kind == EntryKind::Dir) {
        if !old_dirs.contains(&r.rel_path) {
            state.expanded.insert(r.rel_path.clone());
        }
    }
    if let Some(LoadedSession::Dir { arena, stats, .. }) = &mut state.session {
        *arena = result.arena;
        *stats = result.stats;
    }
}

fn dir_left_path(state: &ScriptState, row: &DirNode) -> VfsPath {
    let (lp, ..) = state.dir_parts().expect("目录会话");
    if row.rel_path.is_empty() {
        lp
    } else {
        lp.join(&row.rel_path)
    }
}

fn dir_right_path(state: &ScriptState, row: &DirNode) -> VfsPath {
    let (_, rp, ..) = state.dir_parts().expect("目录会话");
    if row.rel_path.is_empty() {
        rp
    } else {
        rp.join(&row.rel_path)
    }
}

// ---------------------------------------------------------------------------
// 文件操作（COPY/MOVE/DELETE/MKDIR/RENAME/TOUCH/ATTRIB）
// ---------------------------------------------------------------------------

async fn run_ops(
    state: &mut ScriptState,
    ops: &[FileOp],
    outcome: &mut ScriptOutcome,
) -> Result<(), ScriptError> {
    if ops.is_empty() {
        return Ok(());
    }
    let Some((lp, rp, ..)) = state.dir_parts() else {
        return Err(ScriptError::Runtime("操作命令需要 LOAD 文件夹对".into()));
    };
    let report = execute_ops(
        &state.vfs,
        &(lp, rp),
        ops,
        &ExecuteOptions { dry_run: false },
        None,
        &CancelFlag::new(),
    )
    .await;
    for f in &report.succeeded {
        outcome.status.push(format!("OK {}", f.op.describe()));
    }
    for f in &report.skipped {
        outcome.status.push(format!(
            "SKIP {}（{}）",
            f.op.describe(),
            f.detail.clone().unwrap_or_default()
        ));
    }
    for f in &report.failed {
        outcome.errors.push(format!(
            "FAILED {}（{}）",
            f.op.describe(),
            f.detail.clone().unwrap_or_default()
        ));
    }
    if !report.is_all_ok() {
        Err(ScriptError::Runtime(format!(
            "{} 项操作失败",
            report.failed.len()
        )))
    } else {
        rescan_dir(state).await;
        Ok(())
    }
}

async fn exec_cross_ops(
    state: &mut ScriptState,
    dir: SideWord,
    is_move: bool,
    outcome: &mut ScriptOutcome,
) -> Result<(), ScriptError> {
    let arena = match &state.session {
        Some(LoadedSession::Dir { arena, .. }) => arena.clone(),
        _ => {
            return Err(ScriptError::Runtime(
                "COPY/MOVE 需要先 LOAD 文件夹对".into(),
            ))
        }
    };
    let (from, to) = match dir {
        SideWord::LeftToRight => (ai_dir::execute::Side::Left, ai_dir::execute::Side::Right),
        SideWord::RightToLeft => (ai_dir::execute::Side::Right, ai_dir::execute::Side::Left),
    };
    let selected = selected_indices(state, &arena);
    let ops: Vec<FileOp> = selected
        .iter()
        .filter_map(|&i| {
            let row = &arena[i];
            let has_from = match from {
                ai_dir::execute::Side::Left => row.left.is_some(),
                ai_dir::execute::Side::Right => row.right.is_some(),
            };
            if !has_from || row.rel_path.is_empty() {
                return None;
            }
            Some(if is_move {
                FileOp::Move {
                    path: row.rel_path.clone(),
                    target_path: None,
                    from,
                    to,
                }
            } else {
                FileOp::Copy {
                    path: row.rel_path.clone(),
                    target_path: None,
                    from,
                    to,
                }
            })
        })
        .collect();
    run_ops(state, &ops, outcome).await
}

async fn exec_to_path(
    state: &mut ScriptState,
    side: SideSel,
    path: &str,
    is_move: bool,
    outcome: &mut ScriptOutcome,
) -> Result<(), ScriptError> {
    let arena = match &state.session {
        Some(LoadedSession::Dir { arena, .. }) => arena.clone(),
        _ => {
            return Err(ScriptError::Runtime(
                "COPYTO/MOVETO 需要先 LOAD 文件夹对".into(),
            ))
        }
    };
    let selected = selected_indices(state, &arena);
    let dst = PathBuf::from(path);
    std::fs::create_dir_all(&dst)
        .map_err(|e| ScriptError::Runtime(format!("创建目标目录失败 {path}: {e}")))?;
    let mut errors = 0usize;
    for &i in &selected {
        let row = &arena[i];
        if row.kind != EntryKind::File || row.rel_path.is_empty() {
            continue;
        }
        let sides: Vec<ai_dir::execute::Side> = match side {
            SideSel::Left => vec![ai_dir::execute::Side::Left],
            SideSel::Right => vec![ai_dir::execute::Side::Right],
            SideSel::All => vec![ai_dir::execute::Side::Left, ai_dir::execute::Side::Right],
        };
        for s in sides {
            let has = match s {
                ai_dir::execute::Side::Left => row.left.is_some(),
                ai_dir::execute::Side::Right => row.right.is_some(),
            };
            if !has {
                continue;
            }
            let src = match s {
                ai_dir::execute::Side::Left => dir_left_path(state, row),
                ai_dir::execute::Side::Right => dir_right_path(state, row),
            };
            let Ok(src) = std::fs::read(src.as_str()) else {
                errors += 1;
                continue;
            };
            // 保持相对层级
            let rel = row.rel_path.rsplit_once('/').map(|(d, _)| d.to_string());
            let target_dir = match &rel {
                Some(d) => dst.join(d),
                None => dst.clone(),
            };
            if std::fs::create_dir_all(&target_dir).is_err() {
                errors += 1;
                continue;
            }
            let name = row.rel_path.rsplit('/').next().unwrap_or(&row.rel_path);
            let target = target_dir.join(name);
            if std::fs::write(&target, &src).is_err() {
                errors += 1;
                continue;
            }
            outcome
                .status
                .push(format!("OK 复制到 {path}: {}", row.rel_path));
            if is_move {
                // MOVETO 语义：复制后删源
                let del = execute_ops(
                    &state.vfs,
                    &dir_roots(state),
                    &[FileOp::Delete {
                        path: row.rel_path.clone(),
                        side: s,
                    }],
                    &ExecuteOptions { dry_run: false },
                    None,
                    &CancelFlag::new(),
                )
                .await;
                if del.failed.is_empty() {
                    outcome
                        .status
                        .push(format!("OK 移动（源已删）: {}", row.rel_path));
                }
            }
        }
    }
    if errors > 0 {
        Err(ScriptError::Runtime(format!("{errors} 项复制失败")))
    } else {
        rescan_dir(state).await;
        Ok(())
    }
}

async fn exec_side_delete(
    state: &mut ScriptState,
    side: SideSel,
    outcome: &mut ScriptOutcome,
) -> Result<(), ScriptError> {
    let arena = match &state.session {
        Some(LoadedSession::Dir { arena, .. }) => arena.clone(),
        _ => return Err(ScriptError::Runtime("DELETE 需要先 LOAD 文件夹对".into())),
    };
    let selected = selected_indices(state, &arena);
    let sides: Vec<ai_dir::execute::Side> = match side {
        SideSel::Left => vec![ai_dir::execute::Side::Left],
        SideSel::Right => vec![ai_dir::execute::Side::Right],
        SideSel::All => vec![ai_dir::execute::Side::Left, ai_dir::execute::Side::Right],
    };
    let mut ops = Vec::new();
    for &i in &selected {
        let row = &arena[i];
        if row.rel_path.is_empty() {
            continue;
        }
        for s in &sides {
            let has = match s {
                ai_dir::execute::Side::Left => row.left.is_some(),
                ai_dir::execute::Side::Right => row.right.is_some(),
            };
            if has {
                ops.push(FileOp::Delete {
                    path: row.rel_path.clone(),
                    side: *s,
                });
            }
        }
    }
    run_ops(state, &ops, outcome).await
}

async fn exec_mkdir(
    state: &mut ScriptState,
    side: SideSel,
    path: &str,
    outcome: &mut ScriptOutcome,
) -> Result<(), ScriptError> {
    let (lp, rp, ..) = state
        .dir_parts()
        .ok_or_else(|| ScriptError::Runtime("MKDIR 需要先 LOAD 文件夹对".into()))?;
    let sides: Vec<ai_dir::execute::Side> = match side {
        SideSel::Left => vec![ai_dir::execute::Side::Left],
        SideSel::Right => vec![ai_dir::execute::Side::Right],
        SideSel::All => vec![ai_dir::execute::Side::Left, ai_dir::execute::Side::Right],
    };
    let mut ops = Vec::new();
    for s in sides {
        ops.push(FileOp::Mkdir {
            path: path.to_string(),
            side: s,
        });
    }
    let _ = (lp, rp);
    run_ops(state, &ops, outcome).await
}

/// BC `$n` 模板适配：`$<数字>` 后紧跟 [A-Za-z0-9_] 时补 `{}` 边界
/// （`$2_$1.txt` -> `${2}_${1}.txt`；已带花括号的不动）
pub(crate) fn adapt_bc_template(template: &str) -> String {
    let mut out = String::with_capacity(template.len() + 8);
    let b = template.as_bytes();
    let mut i = 0usize;
    while i < b.len() {
        if b[i] == b'$' && i + 1 < b.len() && b[i + 1].is_ascii_digit() {
            let mut j = i + 1;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            // 后一字符是组名字符（字母/数字/下划线）则需要花括号边界
            if j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                // `$2` 后跟组名字符 -> `${2}`（花括号内只放组号，去掉 $ 前缀）
                out.push_str("${");
                out.push_str(&template[i + 1..j]);
                out.push('}');
                i = j;
                continue;
            }
        }
        // 其他字符（含 `${n}`、`$$`）原样复制到下一个字符
        let ch_len = template[i..]
            .chars()
            .next()
            .map(|c| c.len_utf8())
            .unwrap_or(1);
        out.push_str(&template[i..i + ch_len]);
        i += ch_len;
    }
    out
}

async fn exec_rename_regex(
    state: &mut ScriptState,
    re: &regex::Regex,
    template: &str,
    outcome: &mut ScriptOutcome,
) -> Result<(), ScriptError> {
    let arena = match &state.session {
        Some(LoadedSession::Dir { arena, .. }) => arena.clone(),
        _ => return Err(ScriptError::Runtime("RENAME 需要先 LOAD 文件夹对".into())),
    };
    let selected = selected_indices(state, &arena);
    let mut ops = Vec::new();
    for &i in &selected {
        let row = &arena[i];
        if row.rel_path.is_empty() {
            continue;
        }
        // 正则不匹配的文件名跳过（保留原名，不计入操作）；模板 $1.. 捕获组引用
        let new_name = re.replace(&row.name, template).to_string();
        if new_name == row.name || new_name.is_empty() {
            continue;
        }
        for side in [ai_dir::execute::Side::Left, ai_dir::execute::Side::Right] {
            let has = match side {
                ai_dir::execute::Side::Left => row.left.is_some(),
                ai_dir::execute::Side::Right => row.right.is_some(),
            };
            if !has {
                continue;
            }
            ops.push(FileOp::Rename {
                path: row.rel_path.clone(),
                side,
                new_name: new_name.clone(),
            });
        }
    }
    run_ops(state, &ops, outcome).await
}

async fn exec_rename(
    state: &mut ScriptState,
    regexpr: Option<String>,
    pattern: &str,
    outcome: &mut ScriptOutcome,
) -> Result<(), ScriptError> {
    if let Some(rx_pat) = regexpr {
        // 正则重命名（FR-DIR-064 regexpr 形态）：旧掩码 = 文件名正则（捕获组），
        // 新名模板 = regex crate 替换语法（$1/$2 组引用，与 BC $2$1.txt 一致）。
        // 不匹配的文件名跳过（保留原名，不计入操作）。
        let re = regex::Regex::new(&rx_pat)
            .map_err(|e| ScriptError::Runtime(format!("regexpr 非法（{rx_pat}）: {e}")))?;
        // BC 模板适配：regex crate 把 `$2_` 解析为命名组 "2_"（下划线并入组名），
        // BC 语义是「组2 + 字面下划线」——把后跟组名字符的 $<数字> 转成 ${<数字>}
        let template = adapt_bc_template(pattern);
        return exec_rename_regex(state, &re, &template, outcome).await;
    }
    let arena = match &state.session {
        Some(LoadedSession::Dir { arena, .. }) => arena.clone(),
        _ => return Err(ScriptError::Runtime("RENAME 需要先 LOAD 文件夹对".into())),
    };
    let selected = selected_indices(state, &arena);
    let mut ops = Vec::new();
    for &i in &selected {
        let row = &arena[i];
        if row.rel_path.is_empty() {
            continue;
        }
        for s in [ai_dir::execute::Side::Left, ai_dir::execute::Side::Right] {
            let has = match s {
                ai_dir::execute::Side::Left => row.left.is_some(),
                ai_dir::execute::Side::Right => row.right.is_some(),
            };
            if !has {
                continue;
            }
            if let Some(new_name) = apply_dos_mask(pattern, &row.name) {
                ops.push(FileOp::Rename {
                    path: row.rel_path.clone(),
                    side: s,
                    new_name,
                });
            }
        }
    }
    run_ops(state, &ops, outcome).await
}

/// DOS 掩码改名（BC 语义子集）：`?` 复制原字符、`*` 复制余下全部、
/// 字面字符按位对齐复制（`.` 后缀对齐：掩码点号前后段分别对齐源名点号前后段）。
pub fn apply_dos_mask(pattern: &str, name: &str) -> Option<String> {
    // 按最后一个点切分主干与扩展名（无点 = 全为主干）
    let (src_stem, src_ext) = split_stem_ext(name);
    let (mask_stem, mask_ext) = split_stem_ext(pattern);
    let stem = apply_mask_part(mask_stem, src_stem)?;
    // 掩码无点段 = 不保留扩展名（BC 掩码按点段对齐语义）
    let ext = mask_ext.and_then(|m| apply_mask_part(m, src_ext.unwrap_or_default()));
    match ext {
        Some(e) if !e.is_empty() => Some(format!("{stem}.{e}")),
        _ => Some(stem),
    }
}

fn split_stem_ext(s: &str) -> (&str, Option<&str>) {
    match s.rsplit_once('.') {
        Some((a, b)) if !a.is_empty() => (a, Some(b)),
        _ => (s, None),
    }
}

fn apply_mask_part(mask: &str, src: &str) -> Option<String> {
    let chars: Vec<char> = src.chars().collect();
    let mut pos = 0usize;
    let mut out = String::new();
    for c in mask.chars() {
        match c {
            '?' => {
                let ch = chars.get(pos).copied()?;
                out.push(ch);
                pos += 1;
            }
            '*' => {
                out.push_str(&chars[pos..].iter().collect::<String>());
                pos = chars.len();
            }
            other => out.push(other),
        }
    }
    // 掩码耗尽后剩余源字符丢弃（BC 掩码改名语义）
    Some(out)
}

async fn exec_touch(
    state: &mut ScriptState,
    copy: Option<SideWord>,
    set: Option<(SideSel, String)>,
    outcome: &mut ScriptOutcome,
) -> Result<(), ScriptError> {
    let arena = match &state.session {
        Some(LoadedSession::Dir { arena, .. }) => arena.clone(),
        _ => return Err(ScriptError::Runtime("TOUCH 需要先 LOAD 文件夹对".into())),
    };
    let selected = selected_indices(state, &arena);
    let mut ops = Vec::new();
    if let Some(dir) = copy {
        // 复制时间戳：touch left->right => 目标侧 right 取对侧时间
        let target = match dir {
            SideWord::LeftToRight => ai_dir::execute::Side::Right,
            SideWord::RightToLeft => ai_dir::execute::Side::Left,
        };
        for &i in &selected {
            let row = &arena[i];
            if row.kind == EntryKind::File
                && row.left.is_some()
                && row.right.is_some()
                && !row.rel_path.is_empty()
            {
                ops.push(FileOp::Touch {
                    path: row.rel_path.clone(),
                    target_path: None,
                    side: target,
                    mtime_ms: 0,
                });
            }
        }
    } else if let Some((side, what)) = set {
        let mtime_ms = if what.eq_ignore_ascii_case("now") {
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0)
        } else {
            parse_touch_timestamp(&what).ok_or_else(|| {
                ScriptError::Syntax(format!(
                    "touch 时间戳不合法（yyyy-mm-dd[ hh:mm[:ss]]）: {what}"
                ))
            })?
        };
        let sides: Vec<ai_dir::execute::Side> = match side {
            SideSel::Left => vec![ai_dir::execute::Side::Left],
            SideSel::Right => vec![ai_dir::execute::Side::Right],
            SideSel::All => vec![ai_dir::execute::Side::Left, ai_dir::execute::Side::Right],
        };
        for &i in &selected {
            let row = &arena[i];
            if row.rel_path.is_empty() {
                continue;
            }
            for s in &sides {
                let has = match s {
                    ai_dir::execute::Side::Left => row.left.is_some(),
                    ai_dir::execute::Side::Right => row.right.is_some(),
                };
                if has {
                    ops.push(FileOp::Touch {
                        path: row.rel_path.clone(),
                        target_path: None,
                        side: *s,
                        mtime_ms,
                    });
                }
            }
        }
    } else {
        return Err(ScriptError::Syntax("touch 缺少方向或设置参数".into()));
    }
    run_ops(state, &ops, outcome).await
}

/// `yyyy-mm-dd[ hh:mm[:ss]]` -> epoch 毫秒（UTC 语义；宽松分隔符）
pub fn parse_touch_timestamp(v: &str) -> Option<u64> {
    // 日期分隔（/ 与 .）归一化为 -；时间保留冒号
    let normalized: String = v
        .chars()
        .map(|c| if c == '/' || c == '.' { '-' } else { c })
        .collect();
    let parts: Vec<&str> = normalized.split_whitespace().collect();
    let date = parts.first()?;
    let mut dp = date.split('-');
    let y: i64 = dp.next()?.parse().ok()?;
    let mo: u32 = dp.next()?.parse().ok()?;
    let d: u32 = dp.next()?.parse().ok()?;
    let (h, mi, s) = match parts.get(1) {
        Some(t) => {
            let mut tp = t.split(':');
            let h: u32 = tp.next()?.parse().ok()?;
            let mi: u32 = tp.next().unwrap_or("0").parse().ok()?;
            let s: u32 = tp.next().unwrap_or("0").parse().ok()?;
            (h, mi, s)
        }
        None => (0, 0, 0),
    };
    // days_from_civil（Howard Hinnant）
    let y_adj = if mo <= 2 { y - 1 } else { y };
    let era = if y_adj >= 0 { y_adj } else { y_adj - 399 } / 400;
    let yoe = y_adj - era * 400;
    let mp = if mo > 2 { mo - 3 } else { mo + 9 } as i64;
    let doy = (153 * mp + 2) / 5 + (d as i64) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86_400 + (h as i64) * 3600 + (mi as i64) * 60 + (s as i64);
    if secs < 0 {
        return None;
    }
    Some((secs as u64) * 1000)
}

async fn exec_attrib(
    state: &mut ScriptState,
    set: bool,
    letters: &str,
    _paths: &[String],
    outcome: &mut ScriptOutcome,
) -> Result<(), ScriptError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if letters.chars().any(|c| c != 'r') {
            outcome.errors.push(format!(
                "提示: attrib {letters} 含非只读 DOS 属性位（当前平台仅 r 只读位生效）"
            ));
        }
        let arena = match &state.session {
            Some(LoadedSession::Dir { arena, .. }) => arena.clone(),
            _ => return Err(ScriptError::Runtime("ATTRIB 需要先 LOAD 文件夹对".into())),
        };
        let selected = selected_indices(state, &arena);
        let mut ok = 0usize;
        let mut fail = 0usize;
        for &i in &selected {
            let row = &arena[i];
            if row.kind != EntryKind::File || row.rel_path.is_empty() {
                continue;
            }
            let p = dir_left_path(state, row);
            let Ok(Some(path_str)) =
                std::fs::metadata(p.as_str()).map(|_| Some(p.as_str().to_string()))
            else {
                fail += 1;
                continue;
            };
            let meta = std::fs::metadata(&path_str);
            if let Ok(m) = meta {
                let mut perms = m.permissions();
                let mode = perms.mode();
                let new_mode = if set {
                    mode & !0o222 // +r：去写位
                } else {
                    mode | 0o200 // -r：恢复属主写位
                };
                perms.set_mode(new_mode);
                if std::fs::set_permissions(&path_str, perms).is_ok() {
                    ok += 1;
                } else {
                    fail += 1;
                }
            } else {
                fail += 1;
            }
        }
        outcome
            .status
            .push(format!("OK attrib {letters}: {ok} 项（失败 {fail}）"));
        if fail > 0 {
            return Err(ScriptError::Runtime(format!("attrib {fail} 项失败")));
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = (state, set, letters, outcome);
        Err(ScriptError::Runtime(
            "ATTRIB 仅在 Windows/Unix 只读位近似支持".into(),
        ))
    }
}

// ---------------------------------------------------------------------------
// SYNC / SNAPSHOT
// ---------------------------------------------------------------------------

async fn exec_sync(
    state: &mut ScriptState,
    kind: SideWord,
    update: bool,
    all: bool,
    visible: bool,
    create_empty: bool,
    outcome: &mut ScriptOutcome,
) -> Result<(), ScriptError> {
    let (lp, rp, ..) = state
        .dir_parts()
        .ok_or_else(|| ScriptError::Runtime("SYNC 需要先 LOAD 文件夹对".into()))?;
    let Some(LoadedSession::Dir { arena, .. }) = &state.session else {
        return Err(ScriptError::Runtime("SYNC 需要目录会话".into()));
    };
    let rows: Vec<ai_protocol::DirRow> = arena
        .iter()
        .skip(1)
        .filter(|r| if visible { state.row_shown(r) } else { true })
        .map(|r| ai_protocol::DirRow {
            id: r.rel_path.clone(),
            name: r.name.clone(),
            kind: r.kind,
            left: r.left.clone(),
            right: r.right.clone(),
            status: r.status,
            has_children: r.kind == EntryKind::Dir,
            hidden: false,
            suppressed: false,
            left_path: r.left.as_ref().map(|_| r.rel_path.clone()),
            right_path: r.right.as_ref().map(|_| r.rel_path.clone()),
        })
        .collect();
    let selection: Option<Vec<String>> = if state.select_masks.is_empty() {
        None
    } else {
        Some(
            selected_indices(state, arena)
                .iter()
                .map(|&i| arena[i].rel_path.clone())
                .collect(),
        )
    };
    let preset = match (update, all, kind) {
        (true, true, _) => Some(SyncPreset::UpdateBoth),
        (true, false, SideWord::LeftToRight) => Some(SyncPreset::UpdateLeftToRight),
        (true, false, SideWord::RightToLeft) => Some(SyncPreset::UpdateRightToLeft),
        (false, _, SideWord::LeftToRight) => Some(SyncPreset::MirrorLeftToRight),
        (false, _, SideWord::RightToLeft) => Some(SyncPreset::MirrorRightToLeft),
    };
    let strategy = SyncStrategy {
        sync_empty_dirs: create_empty,
        ..SyncStrategy::default()
    };
    let report = run_sync(
        &state.vfs,
        &(lp, rp),
        &rows,
        &SyncRequest {
            preset,
            rules: None,
            strategy,
            selection: selection.as_deref(),
            dry_run: false,
        },
        None,
        &CancelFlag::new(),
    )
    .await;
    outcome.status.push(report.batch.summary_line(if update {
        "sync(update)"
    } else {
        "sync(mirror)"
    }));
    let _ = kind;
    if report.batch.stats.failed > 0 {
        return Err(ScriptError::Runtime(format!(
            "SYNC {} 项失败",
            report.batch.stats.failed
        )));
    }
    rescan_dir(state).await;
    Ok(())
}

fn exec_snapshot(
    state: &mut ScriptState,
    side: SideSel,
    path: Option<String>,
    no_filters: bool,
    output: &str,
    _opts: &ScriptOptions,
    outcome: &mut ScriptOutcome,
) -> Result<(), ScriptError> {
    let mut lines = String::from("{\"magic\":\"ai-compare-snapshot\",\"version\":1}\n");
    if let Some(single) = path {
        // path:<目录> 单侧快照（无需加载会话；递归列举）
        let root = Path::new(&single);
        if !root.is_dir() {
            return Err(ScriptError::LoadFail(format!(
                "快照 path 不存在或不是目录: {single}"
            )));
        }
        let mut rows: Vec<(String, String, u64, u64)> = Vec::new();
        walk_tree(root, root, &mut rows);
        for (rel, kind, size, mtime) in rows {
            lines.push_str(&format!(
                "{{\"path\":\"{}\",\"kind\":\"{}\",\"size\":{},\"mtimeMs\":{}}}\n",
                rel.replace('\\', "\\\\").replace('"', "\\\""),
                kind,
                size,
                mtime
            ));
        }
        let target = resolve_snapshot_target(output);
        std::fs::write(&target, lines.as_bytes())
            .map_err(|e| ScriptError::Runtime(format!("快照写入失败 {target}: {e}")))?;
        outcome.status.push(format!("OK 快照已保存: {target}"));
        return Ok(());
    }
    let Some(LoadedSession::Dir { arena, .. }) = &state.session else {
        return Err(ScriptError::Runtime(
            "SNAPSHOT 需要先 LOAD 文件夹对（或用 path:）".into(),
        ));
    };
    for i in selected_indices(state, arena) {
        let row = &arena[i];
        if row.rel_path.is_empty() {
            continue;
        }
        let info = match side {
            SideSel::Left => row.left.as_ref(),
            SideSel::Right => row.right.as_ref(),
            SideSel::All => row.left.as_ref().or(row.right.as_ref()),
        };
        let Some(info) = info else { continue };
        if !no_filters && !state.row_shown(row) {
            continue;
        }
        lines.push_str(&format!(
            "{{\"path\":\"{}\",\"kind\":\"{}\",\"size\":{},\"mtimeMs\":{}}}\n",
            row.rel_path.replace('\\', "\\\\").replace('"', "\\\""),
            match row.kind {
                EntryKind::File => "file",
                EntryKind::Dir => "dir",
                EntryKind::Symlink => "symlink",
            },
            info.size,
            info.mtime_ms
        ));
    }
    let target = resolve_snapshot_target(output);
    std::fs::write(&target, lines.as_bytes())
        .map_err(|e| ScriptError::Runtime(format!("快照写入失败 {target}: {e}")))?;
    outcome.status.push(format!("OK 快照已保存: {target}"));
    Ok(())
}

/// SNAPSHOT 输出路径规则（FR-SCR-002）：目标为文件夹 -> folder_<date>.bcss；
/// 无扩展名补 .bcss；重名自动 -N 防覆盖
fn resolve_snapshot_target(output: &str) -> String {
    let mut target = output.to_string();
    if target.ends_with('/') || Path::new(&target).is_dir() {
        let (y, mo, d, ..) = now_parts();
        target = format!(
            "{}/folder_{y:04}-{mo:02}-{d:02}.bcss",
            target.trim_end_matches('/')
        );
    }
    if Path::new(&target).extension().is_none() {
        target.push_str(".bcss");
    }
    let mut final_target = target.clone();
    let mut n = 1u32;
    while Path::new(&final_target).exists() {
        let stem = target.strip_suffix(".bcss").unwrap_or(&target);
        final_target = format!("{stem}-{n}.bcss");
        n += 1;
    }
    final_target
}

/// 单侧目录递归列举（快照 path: 变体；名字典序确定性）
fn walk_tree(root: &Path, dir: &Path, out: &mut Vec<(String, String, u64, u64)>) {
    let mut entries: Vec<std::fs::DirEntry> = match std::fs::read_dir(dir) {
        Ok(rd) => rd.filter_map(|e| e.ok()).collect(),
        Err(_) => return,
    };
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let path = e.path();
        let rel = path
            .strip_prefix(root)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        let Ok(meta) = e.metadata() else { continue };
        if meta.is_dir() {
            out.push((rel, "dir".into(), 0, mtime_of(&meta)));
            walk_tree(root, &path, out);
        } else {
            out.push((rel, "file".into(), meta.len(), mtime_of(&meta)));
        }
    }
}

fn mtime_of(meta: &std::fs::Metadata) -> u64 {
    use std::time::UNIX_EPOCH;
    meta.modified()
        .ok()
        .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// 报告命令
// ---------------------------------------------------------------------------

fn report_output_target(
    spec: &ReportSpec,
    naming: &ReportNaming,
    default_ext: &str,
) -> (String, ReportTarget) {
    let raw = spec
        .output_to
        .clone()
        .unwrap_or_else(|| "printer".to_string());
    match raw.to_ascii_lowercase().as_str() {
        "printer" => ("stdout".into(), ReportTarget::Stdout),
        "clipboard" => ("stdout".into(), ReportTarget::Stdout), // 无头环境：剪贴板近似为 stdout
        _ => {
            let expanded = ai_core::expand_template(&raw, naming);
            let path = if Path::new(&expanded).extension().is_none() {
                format!("{expanded}.{default_ext}")
            } else {
                expanded
            };
            let display = path.clone();
            (display, ReportTarget::File(path))
        }
    }
}

enum ReportTarget {
    Stdout,
    File(String),
}

fn emit_report(
    content: String,
    target: ReportTarget,
    outcome: &mut ScriptOutcome,
) -> Result<(), ScriptError> {
    match target {
        ReportTarget::Stdout => {
            print!("{content}");
            let _ = std::io::stdout().flush();
        }
        ReportTarget::File(path) => {
            std::fs::write(&path, content.as_bytes())
                .map_err(|e| ScriptError::Runtime(format!("报告写入失败 {path}: {e}")))?;
            outcome.status.push(format!("OK 报告已写出: {path}"));
        }
    }
    Ok(())
}

fn naming_for(state: &ScriptState, kind: &str) -> ReportNaming {
    let (l, r) = match state.session {
        Some(LoadedSession::Dir {
            ref left_s,
            ref right_s,
            ..
        }) => (left_s.clone(), right_s.clone()),
        Some(LoadedSession::Files {
            ref left_s,
            ref right_s,
            ..
        }) => (left_s.clone(), right_s.clone()),
        None => (String::new(), String::new()),
    };
    ReportNaming {
        now_ms: SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
        left_name: Path::new(&l)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string(),
        right_name: Path::new(&r)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string(),
        kind: kind.to_string(),
    }
}

fn spec_to_options(spec: &ReportSpec, layout: ReportLayout) -> ReportOptions {
    ReportOptions {
        title: spec.title.clone(),
        layout,
        display: spec.display.unwrap_or_default(),
        dir_display: spec.dir_display.unwrap_or_default(),
        ignore_unimportant: spec.ignore_unimportant,
        line_numbers: spec.line_numbers,
        context_lines: spec.context.unwrap_or(ai_core::DEFAULT_REPORT_CONTEXT),
        patch_style: spec.patch_style.unwrap_or_default(),
        delimiter: if spec.tsv { '\t' } else { ',' },
        ..ReportOptions::default()
    }
}

fn require_dir_snapshot(state: &ScriptState) -> Result<ReportSnapshot, ScriptError> {
    let Some(LoadedSession::Dir {
        arena,
        stats,
        left_s,
        right_s,
        ..
    }) = &state.session
    else {
        return Err(ScriptError::Runtime(
            "FOLDER-REPORT 需要先 LOAD 文件夹对".into(),
        ));
    };
    let rows = arena
        .iter()
        .skip(1)
        .filter(|r| state.row_shown(r))
        .map(crate::report_cmd::dir_node_to_report_row)
        .collect();
    Ok(ReportSnapshot::Dir(DirSnapshot {
        left_label: left_s.clone(),
        right_label: right_s.clone(),
        rows,
        stats: ai_protocol_dir_stats_to_report(stats),
    }))
}

fn ai_protocol_dir_stats_to_report(s: &ai_protocol::DirStats) -> ai_core::DirReportStats {
    ai_core::DirReportStats {
        total: s.total,
        same: s.same,
        quick_diff: s.quick_diff,
        content_diff: s.content_diff,
        left_only: s.left_only,
        right_only: s.right_only,
        dir_same: s.dir_same,
        dir_diff: s.dir_diff,
    }
}

fn exec_folder_report(
    state: &ScriptState,
    spec: ReportSpec,
    _opts: &ScriptOptions,
    outcome: &mut ScriptOutcome,
) -> Result<(), ScriptError> {
    let layout = spec.layout.unwrap_or(ReportLayout::Summary);
    let snap = require_dir_snapshot(state)?;
    let naming = naming_for(state, "folder");
    let (path, target) = report_output_target(&spec, &naming, ai_core::layout_extension(layout));
    let _ = path;
    let content = ai_core::report::render_report(&snap, &spec_to_options(&spec, layout))
        .map_err(|e| ScriptError::Runtime(e.to_string()))?;
    emit_report(content, target, outcome)
}

async fn require_text_snapshot(
    state: &ScriptState,
    ignore_unimportant: bool,
) -> Result<ReportSnapshot, ScriptError> {
    let (lp, rp, ls, rs) = state
        .files_parts()
        .ok_or_else(|| ScriptError::Runtime("TEXT-REPORT 需要先 LOAD 文件对".into()))?;
    build_text_snap(&state.vfs, &lp, &rp, ls, rs, ignore_unimportant).await
}

async fn build_text_snap(
    vfs: &LocalFs,
    lp: &VfsPath,
    rp: &VfsPath,
    left_label: String,
    right_label: String,
    ignore_unimportant: bool,
) -> Result<ReportSnapshot, ScriptError> {
    let lt_text = vfs
        .read_file_text(lp)
        .await
        .map_err(|e| ScriptError::Runtime(format!("读取失败 {lp}: {e}")))?;
    let rt_text = vfs
        .read_file_text(rp)
        .await
        .map_err(|e| ScriptError::Runtime(format!("读取失败 {rp}: {e}")))?;
    let lt = ai_core::LineTable::from_text(&lt_text);
    let rt = ai_core::LineTable::from_text(&rt_text);
    let mut config = ai_core::DiffConfig::default();
    if ignore_unimportant {
        config.importance = ignore_unimportant_model();
    }
    let keyed =
        ai_core::diff_config(&lt, &rt, &config).map_err(|e| ScriptError::Runtime(e.to_string()))?;
    Ok(ReportSnapshot::Text(TextSnapshot::from_tables(
        left_label,
        right_label,
        &lt,
        &rt,
        keyed.output.sections,
    )))
}

async fn exec_text_report(
    state: &ScriptState,
    spec: ReportSpec,
    _opts: &ScriptOptions,
    outcome: &mut ScriptOutcome,
) -> Result<(), ScriptError> {
    let layout = spec.layout.unwrap_or(ReportLayout::Summary);
    let snap = require_text_snapshot(state, spec.ignore_unimportant).await?;
    let naming = naming_for(state, "text");
    let (_, target) = report_output_target(&spec, &naming, ai_core::layout_extension(layout));
    let content = ai_core::report::render_report(&snap, &spec_to_options(&spec, layout))
        .map_err(|e| ScriptError::Runtime(e.to_string()))?;
    emit_report(content, target, outcome)
}

async fn exec_hex_report(
    state: &ScriptState,
    spec: ReportSpec,
    _opts: &ScriptOptions,
    outcome: &mut ScriptOutcome,
) -> Result<(), ScriptError> {
    let layout = spec.layout.unwrap_or(ReportLayout::Summary);
    let (lp, rp, ls, rs) = state
        .files_parts()
        .ok_or_else(|| ScriptError::Runtime("HEX-REPORT 需要先 LOAD 文件对".into()))?;
    let lb = state
        .vfs
        .read_file_bytes(&lp)
        .await
        .map_err(|e| ScriptError::Runtime(format!("读取失败 {lp}: {e}")))?;
    let rb = state
        .vfs
        .read_file_bytes(&rp)
        .await
        .map_err(|e| ScriptError::Runtime(format!("读取失败 {rp}: {e}")))?;
    let model = ai_core::hex::diff_bytes(&lb, &rb, &ai_core::hex::HexOptions::default());
    let (sl, sr) = ai_core::hex::status_maps(&model);
    let snap = ReportSnapshot::Hex(ai_core::HexSnapshot {
        left_label: ls,
        right_label: rs,
        left: lb,
        right: rb,
        model,
        status_left: sl,
        status_right: sr,
    });
    let naming = naming_for(state, "hex");
    let (_, target) = report_output_target(&spec, &naming, ai_core::layout_extension(layout));
    let content = ai_core::report::render_report(&snap, &spec_to_options(&spec, layout))
        .map_err(|e| ScriptError::Runtime(e.to_string()))?;
    emit_report(content, target, outcome)
}

/// 表格扩展名判定（FILE-REPORT 自动选 TABLE-REPORT 的依据）
fn is_table_ext(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".csv") || lower.ends_with(".tsv") || lower.ends_with(".tab")
}

async fn exec_table_report(
    state: &ScriptState,
    spec: ReportSpec,
    _opts: &ScriptOptions,
    outcome: &mut ScriptOutcome,
) -> Result<(), ScriptError> {
    let layout = spec.layout.unwrap_or(ReportLayout::Summary);
    let (lp, rp, ls, rs) = state
        .files_parts()
        .ok_or_else(|| ScriptError::Runtime("TABLE-REPORT 需要先 LOAD 文件对".into()))?;
    let lt_text = state
        .vfs
        .read_file_text(&lp)
        .await
        .map_err(|e| ScriptError::Runtime(format!("读取失败 {lp}: {e}")))?;
    let rt_text = state
        .vfs
        .read_file_text(&rp)
        .await
        .map_err(|e| ScriptError::Runtime(format!("读取失败 {rp}: {e}")))?;
    let mut opts = ai_core::table::TableDiffOptions::default();
    let name = lp.file_name().unwrap_or_default().to_ascii_lowercase();
    if name.ends_with(".tsv") || name.ends_with(".tab") || spec.tsv {
        opts.parse.delimiter = '\t';
    }
    let model = ai_core::table::diff_tables(&lt_text, &rt_text, &opts)
        .map_err(|e| ScriptError::Runtime(format!("表格解析失败: {e}")))?;
    let snap = ReportSnapshot::Table(TableSnapshot {
        left_label: ls,
        right_label: rs,
        columns: model.columns,
        rows: model.rows,
        summary: model.summary,
        has_header: opts.parse.has_header,
    });
    let naming = naming_for(state, "table");
    let (_, target) = report_output_target(&spec, &naming, ai_core::layout_extension(layout));
    let content = ai_core::report::render_report(&snap, &spec_to_options(&spec, layout))
        .map_err(|e| ScriptError::Runtime(e.to_string()))?;
    emit_report(content, target, outcome)
}

/// FILE-REPORT：按文件类型自动选择报告（表格格式 -> 表格报告，其余 -> 文本报告）
async fn exec_file_report(
    state: &ScriptState,
    spec: ReportSpec,
    opts: &ScriptOptions,
    outcome: &mut ScriptOutcome,
) -> Result<(), ScriptError> {
    let Some(LoadedSession::Dir { arena, .. }) = &state.session else {
        // 文件对会话：退化为 TEXT-REPORT
        return exec_text_report(state, spec, opts, outcome).await;
    };
    let selected = selected_indices(state, arena);
    let mut content = String::new();
    let layout = spec.layout.unwrap_or(ReportLayout::Summary);
    for &i in &selected {
        let row = &arena[i];
        if row.kind != EntryKind::File
            || !matches!(
                row.status,
                DirEntryStatus::ContentDiff | DirEntryStatus::QuickDiff
            )
        {
            continue;
        }
        let lp = dir_left_path(state, row);
        let rp = dir_right_path(state, row);
        let name = row.name.clone();
        let (lt_text, rt_text) = match (
            state.vfs.read_file_text(&lp).await,
            state.vfs.read_file_text(&rp).await,
        ) {
            (Ok(l), Ok(r)) => (l, r),
            _ => {
                outcome
                    .status
                    .push(format!("SKIP {}（非文本，逐文件报告跳过）", row.rel_path));
                continue;
            }
        };
        let title = format!("{} ({})", row.rel_path, state.confirm_label());
        let mut spec = spec.clone();
        if spec.title.is_none() {
            spec.title = Some(title);
        }
        let snap = if is_table_ext(&name) {
            let mut topts = ai_core::table::TableDiffOptions::default();
            if name.to_ascii_lowercase().ends_with(".tsv") || name.ends_with(".tab") || spec.tsv {
                topts.parse.delimiter = '\t';
            }
            let model = ai_core::table::diff_tables(&lt_text, &rt_text, &topts)
                .map_err(|e| ScriptError::Runtime(format!("表格解析失败 {name}: {e}")))?;
            ReportSnapshot::Table(TableSnapshot {
                left_label: lp.as_str().to_string(),
                right_label: rp.as_str().to_string(),
                columns: model.columns,
                rows: model.rows,
                summary: model.summary,
                has_header: topts.parse.has_header,
            })
        } else {
            let lt = ai_core::LineTable::from_text(&lt_text);
            let rt = ai_core::LineTable::from_text(&rt_text);
            let mut config = ai_core::DiffConfig::default();
            if spec.ignore_unimportant {
                config.importance.leading_ws_unimportant = true;
                config.importance.embedded_ws_unimportant = true;
                config.importance.trailing_ws_unimportant = true;
                config.importance.case_unimportant = true;
            }
            let keyed = ai_core::diff_config(&lt, &rt, &config)
                .map_err(|e| ScriptError::Runtime(e.to_string()))?;
            ReportSnapshot::Text(TextSnapshot::from_tables(
                lp.as_str(),
                rp.as_str(),
                &lt,
                &rt,
                keyed.output.sections,
            ))
        };
        content.push_str(
            &ai_core::report::render_report(&snap, &spec_to_options(&spec, layout))
                .map_err(|e| ScriptError::Runtime(e.to_string()))?,
        );
        content.push('\n');
    }
    let naming = naming_for(state, "file");
    let (_, target) = report_output_target(&spec, &naming, ai_core::layout_extension(layout));
    emit_report(content, target, outcome)
}

impl ScriptState {
    fn confirm_label(&self) -> &'static str {
        match self.confirm {
            ConfirmMode::Prompt => "prompt",
            ConfirmMode::YesToAll => "yes-to-all",
            ConfirmMode::NoToAll => "no-to-all",
        }
    }
}

// ---------------------------------------------------------------------------
// CLI 入口
// ---------------------------------------------------------------------------

/// `aic script run <file.aicscript> [args...] [--var name=value ...] [--silent]`
pub fn cmd_script(args: &[String]) -> ExitCode {
    match args.first().map(String::as_str) {
        Some("run") => run_cli_script(&args[1..]),
        _ => run_cli_script(args),
    }
}

/// 独立入口（`aiscript` bin / `aic @file` 共用）：
/// args[0] 为脚本路径；其后 token 作为脚本位置参数（`-`/`/` 开头的开关剥离；
/// `--var k=v` 注入变量；`--silent` 静默）。
pub fn run_cli_script(args: &[String]) -> ExitCode {
    let mut positional: Vec<String> = Vec::new();
    let mut vars: Vec<(String, String)> = Vec::new();
    let mut silent = false;
    let mut i = 0usize;
    while i < args.len() {
        let a = &args[i];
        if a == "--silent" {
            silent = true;
        } else if a == "--var" {
            i += 1;
            match args.get(i) {
                Some(kv) => match kv.split_once('=') {
                    Some((k, v)) => vars.push((k.to_string(), v.to_string())),
                    None => {
                        eprintln!("aic script: --var 需要 name=value 形式");
                        return EXIT_ERROR.into();
                    }
                },
                None => {
                    eprintln!("aic script: --var 缺少值");
                    return EXIT_ERROR.into();
                }
            }
        } else if let Some(kv) = a.strip_prefix("--var=") {
            match kv.split_once('=') {
                Some((k, v)) => vars.push((k.to_string(), v.to_string())),
                None => {
                    eprintln!("aic script: --var 需要 name=value 形式");
                    return EXIT_ERROR.into();
                }
            }
        } else {
            positional.push(a.clone());
        }
        i += 1;
    }
    let Some(script_path) = positional.first() else {
        eprintln!("用法: aic script run <file.aicscript> [args...] [--var name=value] [--silent]");
        return EXIT_SCRIPT_SYNTAX.into();
    };
    let src = match std::fs::read_to_string(script_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("aic script: 读脚本失败 {script_path}: {e}");
            return EXIT_SCRIPT_READ.into();
        }
    };
    let script_args = script_positional_args(&positional[1..]);
    let opts = ScriptOptions {
        silent,
        vars,
        cwd: None,
    };
    let outcome = run_script(&src, &script_args, &opts);
    for e in &outcome.errors {
        eprintln!("aic script: {e}");
    }
    outcome.exit_code.into()
}

// ---------------------------------------------------------------------------
// 单测（词法/解析/掩码/命令矩阵/退出码）
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    // ---- 词法 ----

    #[test]
    fn logical_lines_join_continuation_and_strip_comments() {
        let src = "# 注释行\n\nload a &\n  b # 行内注释\nbeep\nunquoted \"has # hash\" arg\n";
        let lines = logical_lines(src);
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert_eq!(lines[0].text, "load a b");
        assert_eq!(lines[0].no, 3);
        assert_eq!(lines[1].text, "beep");
        assert_eq!(lines[2].text, "unquoted \"has # hash\" arg");
    }

    #[test]
    fn tokenize_respects_quotes() {
        let t = tokenize("load \"my folder a\" b");
        assert_eq!(
            t,
            vec![
                "load".to_string(),
                "my folder a".to_string(),
                "b".to_string()
            ]
        );
    }

    #[test]
    fn substitute_positional_env_and_dynamic() {
        let m = vars(&[("CUSTOM", "cx")]);
        std::env::set_var("AIC_SCRIPT_TEST_VAR", "envval");
        let out = substitute_vars(
            "copy %1 %2 %CUSTOM% %AIC_SCRIPT_TEST_VAR% %date% %%",
            &["L".into(), "R".into()],
            &m,
        );
        assert!(out.starts_with("copy L R cx envval "));
        assert!(out.ends_with(" %"), "{out}");
        // %fn_time% 文件名安全（无冒号）
        let out = substitute_vars("report_%fn_time%", &[], &m);
        assert!(!out.contains(':') && out.starts_with("report_"));
    }

    #[test]
    fn positional_args_skip_switches() {
        let toks = vec![
            "a".to_string(),
            "-x".to_string(),
            "/q".to_string(),
            "b".to_string(),
        ];
        // Unix：`-` 开头为开关；`/q` 为合法绝对路径保留（Windows 下 `/` 为开关前缀）
        #[cfg(not(windows))]
        assert_eq!(
            script_positional_args(&toks),
            vec!["a".to_string(), "/q".to_string(), "b".to_string()]
        );
        #[cfg(windows)]
        assert_eq!(
            script_positional_args(&toks),
            vec!["a".to_string(), "b".to_string()]
        );
    }

    // ---- 解析 ----

    fn parse_one(src: &str) -> Cmd {
        let lines = logical_lines(src);
        assert_eq!(lines.len(), 1);
        parse_command(&lines[0]).expect("解析成功")
    }

    #[test]
    fn parse_commands_matrix() {
        assert!(matches!(
            parse_one("LOAD create:all C:\\a D:\\b"),
            Cmd::Load {
                create: Some(_),
                ..
            }
        ));
        assert!(matches!(
            parse_one("select newer.files right.older.files"),
            Cmd::Select { .. }
        ));
        assert!(matches!(
            parse_one("FILTER \"*.txt;-sub\\\""),
            Cmd::Filter { .. }
        ));
        assert!(matches!(
            parse_one("EXPAND all"),
            Cmd::Expand { all: true, .. }
        ));
        assert!(matches!(
            parse_one("collapse sub"),
            Cmd::Collapse { all: false, .. }
        ));
        assert!(matches!(
            parse_one("COMPARE CRC"),
            Cmd::Compare {
                mode: Some(CompareWord::Crc)
            }
        ));
        assert!(matches!(parse_one("COPY left->right"), Cmd::Copy { .. }));
        assert!(matches!(parse_one("moveto right out/"), Cmd::MoveTo { .. }));
        assert!(matches!(
            parse_one("delete recyclebin:no all"),
            Cmd::Delete { .. }
        ));
        assert!(matches!(
            parse_one("rename \"new_?.txt\""),
            Cmd::Rename { regexpr: None, .. }
        ));
        assert!(matches!(
            parse_one("touch left->right"),
            Cmd::Touch {
                copy: Some(_),
                set: None
            }
        ));
        assert!(matches!(
            parse_one("touch all:now"),
            Cmd::Touch { set: Some(_), .. }
        ));
        assert!(matches!(
            parse_one("attrib +h"),
            Cmd::Attrib { set: true, .. }
        ));
        assert!(matches!(
            parse_one("SYNC visible create-empty mirror:right->left"),
            Cmd::Sync { update: false, .. }
        ));
        assert!(matches!(parse_one("beep"), Cmd::Beep));
        assert!(matches!(
            parse_one("LOG none"),
            Cmd::Log {
                level: LogWord::None,
                ..
            }
        ));
        assert!(matches!(
            parse_one("option stop-on-error"),
            Cmd::Option {
                stop_on_error: Some(true),
                ..
            }
        ));
        assert!(matches!(
            parse_one("if-exists x.txt beep"),
            Cmd::IfExists { .. }
        ));
        match parse_one("folder-report layout:side-by-side title:\"R\" output-to:out.html") {
            Cmd::FolderReport { spec } => {
                assert_eq!(spec.layout, Some(ReportLayout::SideBySide));
                assert_eq!(spec.title.as_deref(), Some("R"));
                assert_eq!(spec.output_to.as_deref(), Some("out.html"));
            }
            c => panic!("{c:?}"),
        }
        // 大小写不敏感 + data-report 别名
        assert!(matches!(
            parse_one("DATA-REPORT layout:xml"),
            Cmd::TableReport { .. }
        ));
    }

    #[test]
    fn parse_errors() {
        let lines = logical_lines("bogus-command x");
        assert!(parse_command(&lines[0]).is_err());
        let lines = logical_lines("copy wrongdir");
        assert!(parse_command(&lines[0]).is_err());
        let lines = logical_lines("snapshot");
        assert!(
            parse_command(&lines[0]).is_err(),
            "snapshot 缺 output 应报错"
        );
    }

    #[test]
    fn select_mask_semantics() {
        use ai_protocol::SideInfo;
        let row = |status: DirEntryStatus, l: bool, r: bool| DirNode {
            rel_path: "f".into(),
            name: "f".into(),
            kind: EntryKind::File,
            left: l.then(|| SideInfo::base(1, 300)),
            right: r.then(|| SideInfo::base(1, 200)),
            status,
            parent: None,
            children: Vec::new(),
            expanded: false,
            aligned_with: None,
            right_rel: None,
            absorbed_by: None,
            scan_failed: false,
        };
        let left_only = row(DirEntryStatus::LeftOnly, true, false);
        let diff = row(DirEntryStatus::ContentDiff, true, true);
        let same = row(DirEntryStatus::Same, true, true);
        let m = parse_select_mask("left.orphan.files").unwrap();
        assert!(m.keeps(&left_only));
        assert!(!m.keeps(&diff));
        let m = parse_select_mask("newer.files").unwrap();
        assert!(
            m.keeps(&diff),
            "左 mtime 300 > 右 200 -> 左较新 -> newer.files 保留"
        );
        // newer 是元数据条件：内容相同但 mtime 不同同样命中（BC 语义）
        assert!(m.keeps(&same));
        let m = parse_select_mask("all").unwrap();
        assert!(m.keeps(&same) && m.keeps(&left_only));
    }

    #[test]
    fn dos_mask_rename() {
        assert_eq!(
            apply_dos_mask("new_?.txt", "old.txt"),
            Some("new_o.txt".into())
        );
        assert_eq!(
            apply_dos_mask("*_bak.*", "report.txt"),
            Some("report_bak.txt".into())
        );
        assert_eq!(apply_dos_mask("a?c", ""), None, "? 源耗尽 -> None");
        assert_eq!(
            apply_dos_mask("fixed", "anything.txt"),
            Some("fixed".into())
        );
    }

    #[test]
    fn touch_timestamp_parse() {
        // 2026-01-02 03:04:05 UTC = epoch 1767323045
        assert_eq!(
            parse_touch_timestamp("2026-01-02 03:04:05"),
            Some(1_767_323_045_000)
        );
        assert_eq!(
            parse_touch_timestamp("2026/01/02 3:4"),
            Some(1_767_323_040_000)
        );
        assert_eq!(parse_touch_timestamp("bad"), None);
    }

    // ---- 端到端（语料矩阵） ----

    fn corpus() -> (tempfile::TempDir, String, String) {
        let dir = tempfile::tempdir().unwrap();
        let l = dir.path().join("L");
        let r = dir.path().join("R");
        std::fs::create_dir_all(&l).unwrap();
        std::fs::create_dir_all(&r).unwrap();
        std::fs::write(l.join("same.txt"), "x\n").unwrap();
        std::fs::write(r.join("same.txt"), "x\n").unwrap();
        std::fs::write(l.join("diff.txt"), "a\nb\n").unwrap();
        std::fs::write(r.join("diff.txt"), "a\nB\n").unwrap();
        std::fs::write(l.join("lonly.txt"), "L\n").unwrap();
        std::fs::write(r.join("ronly.txt"), "R\n").unwrap();
        std::fs::write(l.join("t.csv"), "id,v\n1,a\n").unwrap();
        std::fs::write(r.join("t.csv"), "id,v\n1,b\n").unwrap();
        (
            dir,
            l.to_string_lossy().into_owned(),
            r.to_string_lossy().into_owned(),
        )
    }

    fn run(src: &str, vars: &[(&str, &str)]) -> ScriptOutcome {
        run_script(
            src,
            &[],
            &ScriptOptions {
                silent: true,
                vars: vars
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
                cwd: None,
            },
        )
    }

    #[test]
    fn e2e_load_select_folder_report() {
        let (_d, l, r) = corpus();
        let out = run(
            &format!(
                "load {l} {r}\n\
                 select right.orphan.files\n\
                 folder-report layout:list output-to:report.txt\n"
            ),
            &[],
        );
        assert_eq!(out.exit_code, EXIT_SAME, "{out:?}");
        let report = std::fs::read_to_string("report.txt").unwrap();
        let _ = std::fs::remove_file("report.txt");
        // 报告为"显示"口径（filter 生效；select 只作用于操作）——孤项全部列出
        assert!(report.contains("rightOnly\tronly.txt"), "{report}");
        assert!(report.contains("leftOnly\tlonly.txt"));
        assert!(
            report.contains("contentDiff\tdiff.txt"),
            "脚本默认内容判据下 diff.txt 判异: {report}"
        );
    }

    #[test]
    fn e2e_text_report_patch_and_copy_ops() {
        let (_d, l, r) = corpus();
        // 文本报告（文件对）+ patch 布局
        let out = run(
            &format!(
                "load {l}/diff.txt {r}/diff.txt\n\
                 text-report layout:patch output-to:diff.patch\n\
                 load {l} {r}\n\
                 select left.diff.files\n\
                 copy left->right\n"
            ),
            &[],
        );
        assert_eq!(out.exit_code, EXIT_SAME, "{out:?}");
        let patch = std::fs::read_to_string("diff.patch").unwrap();
        let _ = std::fs::remove_file("diff.patch");
        assert!(patch.contains("-b") && patch.contains("+B"), "{patch}");
        // copy left->right 后右侧 diff.txt 应与左侧一致（diff 条件 = 内容差异；孤项不含）
        assert_eq!(
            std::fs::read_to_string(format!("{r}/diff.txt")).unwrap(),
            "a\nb\n"
        );
        assert!(
            !std::path::Path::new(&format!("{r}/lonly.txt")).exists(),
            "left.diff 不含孤项"
        );
        // 孤项复制：left.orphan.files
        let out = run(
            &format!("load {l} {r}\nselect left.orphan.files\ncopy left->right\n"),
            &[],
        );
        assert_eq!(out.exit_code, EXIT_SAME, "{out:?}");
        assert_eq!(
            std::fs::read_to_string(format!("{r}/lonly.txt")).unwrap(),
            "L\n",
            "孤项复制落盘"
        );
    }

    #[test]
    fn e2e_delete_touch_sync_snapshot() {
        let (_d, l, r) = corpus();
        let out = run(
            &format!(
                "load {l} {r}\n\
                 select right.orphan.files\n\
                 delete all\n\
                 select right.diff.files\n\
                 touch left->right\n\
                 select all\n\
                 sync mirror:left->right\n\
                 snapshot output:snap.bcss\n"
            ),
            &[],
        );
        assert_eq!(out.exit_code, EXIT_SAME, "{out:?}");
        assert!(
            !Path::new(&format!("{r}/ronly.txt")).exists(),
            "delete all 删除右侧孤项"
        );
        assert!(Path::new("snap.bcss").exists(), "快照落盘");
        let snap = std::fs::read_to_string("snap.bcss").unwrap();
        let _ = std::fs::remove_file("snap.bcss");
        assert!(snap.contains("\"path\":\"same.txt\""), "{snap}");
        // mirror 后 ronly 应被删、lonly 应被复制进右侧
        assert!(Path::new(&format!("{r}/lonly.txt")).exists());
    }

    #[test]
    fn e2e_table_and_file_report() {
        let (_d, l, r) = corpus();
        let out = run(
            &format!(
                "load {l}/t.csv {r}/t.csv\n\
                 table-report layout:csv output-to:t-report.csv\n\
                 load {l} {r}\n\
                 file-report output-to:files-report.txt\n"
            ),
            &[],
        );
        assert_eq!(out.exit_code, EXIT_SAME, "{out:?}");
        let tr = std::fs::read_to_string("t-report.csv").unwrap();
        assert!(tr.contains("changed,0,0"), "{tr}");
        let fr = std::fs::read_to_string("files-report.txt").unwrap();
        let _ = (
            std::fs::remove_file("t-report.csv"),
            std::fs::remove_file("files-report.txt"),
        );
        assert!(fr.contains("t.csv"), "file-report 含逐文件段: {fr}");
    }

    #[test]
    fn e2e_variables_and_if_exists() {
        let (_d, l, r) = corpus();
        // %1/%2 位置参数 + if-exists 分支控制
        let args = vec![l.clone(), r.clone()];
        let out = run_script(
            "load %1 %2\nif-exists no-such-file.xyz folder-report layout:list output-to:skip.txt\nif-not-exists no-such-file.xyz folder-report layout:stats output-to:report-out.txt\n",
            &args,
            &ScriptOptions { silent: true, ..Default::default() },
        );
        assert_eq!(out.exit_code, EXIT_SAME, "{out:?}");
        assert!(Path::new("report-out.txt").exists());
        assert!(!Path::new("skip.txt").exists(), "if-exists 假分支不应执行");
        let _ = std::fs::remove_file("report-out.txt");
    }

    #[test]
    fn e2e_exit_codes() {
        let (_d, l, r) = corpus();
        // 106 语法错误
        let out = run("not-a-command\n", &[]);
        assert_eq!(out.exit_code, EXIT_SCRIPT_SYNTAX);
        // 107 加载失败
        let out = run(&format!("load {l}/missing-dir {r}\n"), &[]);
        assert_eq!(out.exit_code, EXIT_SCRIPT_LOAD);
        // 105 脚本文件不存在（run_cli_script）
        let code = run_cli_script(&["no-such-script.aicscript".to_string()]);
        assert_eq!(code, ExitCode::from(EXIT_SCRIPT_READ));
        // stop-on-error：运行期失败中止退出 2
        let out = run(
            &format!("option stop-on-error\nload {l} {r}\ncopy left->right\nbogus\n"),
            &[],
        );
        // copy 成功后 bogus 是语法错误（106 立即终止）
        assert_eq!(out.exit_code, EXIT_SCRIPT_SYNTAX);
    }

    #[test]
    fn e2e_log_written() {
        let (_d, l, r) = corpus();
        let out = run(
            &format!("LOG verbose script-test.log\nload {l} {r}\nselect all\n"),
            &[],
        );
        assert_eq!(out.exit_code, EXIT_SAME, "{out:?}");
        let log = std::fs::read_to_string("script-test.log").unwrap();
        let _ = std::fs::remove_file("script-test.log");
        assert!(log.contains("load"), "{log}");
        assert!(log.contains("select"), "{log}");
    }

    /// 日志落盘失败必须上报：此前两个 `.ok()?` 让 write_log 永远返回 None，
    /// 错误分支成死代码，用户以为在记录日志实际什么都没有且退出码 0。
    #[test]
    fn e2e_log_write_failure_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("no_such_parent").join("Log.txt");
        let out = run(
            &format!("LOG normal {}\nbeep\n", bad.to_string_lossy()),
            &[],
        );
        assert_eq!(
            out.exit_code, EXIT_ERROR,
            "日志写失败应报错而非静默成功: {out:?}"
        );
        assert!(
            out.errors.iter().any(|e| e.contains("日志写入失败")),
            "错误信息应指明日志写入失败: {out:?}"
        );
    }

    #[test]
    fn e2e_expand_selects_hidden_subtree() {
        let dir = tempfile::tempdir().unwrap();
        let l = dir.path().join("L");
        let r = dir.path().join("R");
        std::fs::create_dir_all(l.join("sub")).unwrap();
        std::fs::create_dir_all(r.join("sub")).unwrap();
        std::fs::write(l.join("sub").join("deep.txt"), "1\n").unwrap();
        std::fs::write(r.join("sub").join("deep.txt"), "2\n").unwrap();
        let (l, r) = (
            l.to_string_lossy().into_owned(),
            r.to_string_lossy().into_owned(),
        );
        // 未展开：子行不可见 -> 报告为空差异
        let out = run(
            &format!("load {l} {r}\nfolder-report layout:list output-to:r1.txt\n"),
            &[],
        );
        assert_eq!(out.exit_code, EXIT_SAME);
        let rep = std::fs::read_to_string("r1.txt").unwrap();
        let _ = std::fs::remove_file("r1.txt");
        assert!(!rep.contains("deep.txt"), "未展开子项不可见: {rep}");
        // EXPAND 后可见
        let out = run(
            &format!("load {l} {r}\nexpand sub\nfolder-report layout:list output-to:r2.txt\n"),
            &[],
        );
        assert_eq!(out.exit_code, EXIT_SAME);
        let rep = std::fs::read_to_string("r2.txt").unwrap();
        let _ = std::fs::remove_file("r2.txt");
        assert!(rep.contains("deep.txt"), "展开后子项可见: {rep}");
    }

    #[test]
    fn e2e_criteria_affects_compare() {
        let (_d, l, r) = corpus();
        let out = run(
            &format!(
                "load {l} {r}\ncriteria crc\ncompare crc\nfolder-report layout:stats output-to:s.txt\n"
            ),
            &[],
        );
        assert_eq!(out.exit_code, EXIT_SAME, "{out:?}");
        let s = std::fs::read_to_string("s.txt").unwrap();
        let _ = std::fs::remove_file("s.txt");
        assert!(
            s.contains("contentDiff=2"),
            "diff.txt 与 t.csv 均内容判异: {s}"
        );
    }

    #[test]
    fn e2e_view_reports_unsupported_and_mixed_load() {
        let (_d, l, r) = corpus();
        // 文件/目录混载 -> 107
        let out = run(&format!("load {l} {r}/ronly.txt\n"), &[]);
        assert_eq!(out.exit_code, EXIT_SCRIPT_LOAD);
        // media-report：引擎未实现 -> stop-on-error 下退出 2
        let out = run(
            &format!("option stop-on-error\nload {l} {r}\nmedia-report output-to:x.txt\n"),
            &[],
        );
        assert_eq!(out.exit_code, EXIT_ERROR);
        assert!(out.errors.iter().any(|e| e.contains("media")), "{out:?}");
    }

    #[test]
    fn e2e_hex_report_and_moveto() {
        let dir = tempfile::tempdir().unwrap();
        let lb = dir.path().join("l.bin");
        let rb = dir.path().join("r.bin");
        std::fs::write(&lb, b"AAAABBBB").unwrap();
        std::fs::write(&rb, b"AAAAXXBB").unwrap();
        let (ls, rs) = (
            lb.to_string_lossy().into_owned(),
            rb.to_string_lossy().into_owned(),
        );
        let out = run(
            &format!("load {ls} {rs}\nhex-report layout:stats output-to:h.txt\n"),
            &[],
        );
        assert_eq!(out.exit_code, EXIT_SAME, "{out:?}");
        let h = std::fs::read_to_string("h.txt").unwrap();
        let _ = std::fs::remove_file("h.txt");
        assert!(h.contains("changedBytes=2"), "{h}");
    }

    #[test]
    fn e2e_output_template_placeholders() {
        let (_d, l, r) = corpus();
        let out = run(
            &format!(
                "load {l}/diff.txt {r}/diff.txt\ntext-report layout:summary output-to:rep_%k.txt\n"
            ),
            &[],
        );
        assert_eq!(out.exit_code, EXIT_SAME, "{out:?}");
        assert!(Path::new("rep_text.txt").exists(), "%k 模板展开");
        let _ = std::fs::remove_file("rep_text.txt");
    }

    /// 操作后重扫不得扩大用户 EXPAND/COLLAPSE 界定的作用域：
    /// 一次无害 COPY 曾把后续 DELETE 从顶层静默扩到整棵子树，销毁嵌套用户数据。
    #[test]
    fn e2e_rescan_preserves_collapse_scope() {
        let dir = tempfile::tempdir().unwrap();
        let l = dir.path().join("L");
        let r = dir.path().join("R");
        for base in [&l, &r] {
            std::fs::create_dir_all(base.join("nested")).unwrap();
            std::fs::write(base.join("nested").join("deep.txt"), "precious\n").unwrap();
            std::fs::write(base.join("top.txt"), "top\n").unwrap();
        }
        let (ls, rs) = (
            l.to_string_lossy().into_owned(),
            r.to_string_lossy().into_owned(),
        );
        let out = run(
            &format!("load {ls} {rs}\nselect all.files\ncopy left->right\ndelete all\n"),
            &[],
        );
        assert_eq!(out.exit_code, EXIT_SAME, "{out:?}");
        assert!(
            l.join("nested").join("deep.txt").exists()
                && r.join("nested").join("deep.txt").exists(),
            "未 EXPAND 的嵌套文件必须存活（COPY 不得扩大 DELETE 作用域）"
        );
        assert!(!l.join("top.txt").exists(), "顶层可见文件应被删除");
    }

    /// 反向：本次操作新出现的目录仍应自动展开（否则 MKDIR/COPY 产物无法被后续 SELECT 捕获）。
    /// 必须用嵌套新目录：顶层目录的 row_visible 恒真，测不到自动展开路径。
    #[test]
    fn e2e_rescan_expands_newly_created_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let l = dir.path().join("L");
        let r = dir.path().join("R");
        std::fs::create_dir_all(&l).unwrap();
        std::fs::create_dir_all(&r).unwrap();
        std::fs::write(l.join("a.txt"), "a\n").unwrap();
        std::fs::write(r.join("a.txt"), "a\n").unwrap();
        let (ls, rs) = (
            l.to_string_lossy().into_owned(),
            r.to_string_lossy().into_owned(),
        );
        let out = run(
            &format!("load {ls} {rs}\nmkdir all sub\nmkdir all sub/inner\nselect all.files\nfolder-report layout:list output-to:n.txt\n"),
            &[],
        );
        assert_eq!(out.exit_code, EXIT_SAME, "{out:?}");
        let rep = std::fs::read_to_string("n.txt").unwrap();
        let _ = std::fs::remove_file("n.txt");
        assert!(
            rep.contains("sub/inner"),
            "新建嵌套目录应自动展开可见: {rep}"
        );
    }
}
