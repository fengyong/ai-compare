//! `aic report`（P11 / FR-RPT 命令行面）：
//!
//! ```text
//! aic report <left> <right> --layout summary|side-by-side|interleaved|patch|html|xml|csv|tree|list|stats
//!            [-o <out>] [--title <t>] [--display all|mismatches|context|matches]
//!            [--dir-display <过滤器>] [--ignore-unimportant] [--line-numbers]
//!            [-U <n>] [--patch-style unified|context|normal] [--tsv]
//!            [--color|--mono] [--wrap none|character|word] [--wrap-columns <n>]
//!            [--kind auto|dir|text|table|hex] [--file-links] [--rules <path>] [--format-load <path>]
//! ```
//!
//! 输入自动判别（--kind 覆盖）：两侧均为目录 -> 文件夹报告；否则文本报告
//! （--kind table/hex 走对应引擎）。报告写 `-o` 指定文件（支持 %f/%t 等
//! 文件名模板占位符，见 ai_core::expand_template）；无 -o 时写 stdout。
//! 退出码：0 成功（报告已产出，无论有无差异）/ 2 参数或 IO 错误。

use std::io::Write as _;
use std::path::Path;
use std::process::ExitCode;

use ai_core::{
    ColorMode, DirColumns, DirDisplayFilter, DirSnapshot, DisplayFilter, HexSnapshot, PatchStyle,
    ReportEntryKind, ReportEntryStatus, ReportLayout, ReportNaming, ReportOptions, ReportSideInfo,
    ReportSnapshot, TableSnapshot, TextSnapshot, WrapMode,
};
use ai_protocol::CompareCriteria;
use ai_vfs::{RouteFs, VfsPath, VfsProvider as _};

use crate::app::arg_error;
use crate::{EXIT_ERROR, EXIT_SAME};

/// 报告命令参数（解析产物；单测可直接构造）
#[derive(Debug, Clone, Default)]
pub struct ReportFlags {
    pub layout: Option<ReportLayout>,
    pub out: Option<String>,
    pub title: Option<String>,
    pub display: DisplayFilter,
    pub dir_display: DirDisplayFilter,
    pub ignore_unimportant: bool,
    pub line_numbers: bool,
    pub context_lines: u32,
    pub patch_style: PatchStyle,
    pub tsv: bool,
    pub color: Option<ColorMode>,
    pub wrap: WrapMode,
    pub wrap_columns: usize,
    pub columns: DirColumns,
    pub include_file_links: bool,
    pub kind: Option<String>,
    pub rules_path: Option<String>,
    pub format_pack_path: Option<String>,
}

pub fn cmd_report(args: &[String]) -> ExitCode {
    let mut positional: Vec<&String> = Vec::new();
    let mut flags = ReportFlags {
        context_lines: ai_core::DEFAULT_REPORT_CONTEXT,
        wrap_columns: 80,
        ..ReportFlags::default()
    };
    let mut i = 0usize;
    while i < args.len() {
        let a = args[i].as_str();
        let next = |i: &mut usize| -> Option<String> {
            *i += 1;
            args.get(*i).cloned()
        };
        let next_or_eq = |a: &str, prefix: &str| -> Option<String> {
            a.strip_prefix(prefix).map(|rest| rest.to_string())
        };
        match a {
            "--layout" => match next(&mut i).and_then(|v| ReportLayout::parse(&v)) {
                Some(l) => flags.layout = Some(l),
                None => {
                    return arg_error("--layout 仅支持 summary|side-by-side|interleaved|patch|html|xml|csv|tree|list|stats")
                }
            },
            _ if next_or_eq(a, "--layout=").is_some() => {
                let v = next_or_eq(a, "--layout=").unwrap();
                match ReportLayout::parse(&v) {
                    Some(l) => flags.layout = Some(l),
                    None => {
                        return arg_error("--layout 仅支持 summary|side-by-side|interleaved|patch|html|xml|csv|tree|list|stats")
                    }
                }
            }
            "-o" | "--output" => match next(&mut i) {
                Some(p) => flags.out = Some(p),
                None => return arg_error("-o 需要输出路径参数（支持 %f %t 等占位符）"),
            },
            _ if next_or_eq(a, "--output=").is_some() => {
                flags.out = Some(next_or_eq(a, "--output=").unwrap());
            }
            "--title" => match next(&mut i) {
                Some(t) => flags.title = Some(t),
                None => return arg_error("--title 需要标题参数"),
            },
            _ if next_or_eq(a, "--title=").is_some() => {
                flags.title = Some(next_or_eq(a, "--title=").unwrap());
            }
            "--display" => match next(&mut i).as_deref() {
                Some("all") => flags.display = DisplayFilter::All,
                Some("mismatches") => flags.display = DisplayFilter::Mismatches,
                Some("context") => flags.display = DisplayFilter::Context,
                Some("matches") => flags.display = DisplayFilter::Matches,
                _ => return arg_error("--display 仅支持 all|mismatches|context|matches"),
            },
            _ if next_or_eq(a, "--display=").is_some() => {
                let v = next_or_eq(a, "--display=").unwrap();
                flags.display = match v.as_str() {
                    "all" => DisplayFilter::All,
                    "mismatches" => DisplayFilter::Mismatches,
                    "context" => DisplayFilter::Context,
                    "matches" => DisplayFilter::Matches,
                    _ => return arg_error("--display 仅支持 all|mismatches|context|matches"),
                };
            }
            "--dir-display" => {
                let Some(v) = next(&mut i) else { return arg_error("--dir-display 需要过滤器参数") };
                match ai_core::DirDisplayFilter::parse(&v) {
                    Some(f) => flags.dir_display = f,
                    None => return arg_error("--dir-display 过滤器名不合法（all/mismatches/no-orphans/orphans/...）"),
                }
            }
            _ if next_or_eq(a, "--dir-display=").is_some() => {
                let v = next_or_eq(a, "--dir-display=").unwrap();
                match ai_core::DirDisplayFilter::parse(&v) {
                    Some(f) => flags.dir_display = f,
                    None => return arg_error("--dir-display 过滤器名不合法"),
                }
            }
            "--ignore-unimportant" | "-iu" => flags.ignore_unimportant = true,
            "--line-numbers" | "-ln" => flags.line_numbers = true,
            "-U" | "--context" => match next(&mut i).and_then(|v| v.parse::<u32>().ok()) {
                Some(n) => flags.context_lines = n,
                None => return arg_error("-U 需要数字参数"),
            },
            _ if a.starts_with("-U") && a.len() > 2 => match a[2..].parse::<u32>() {
                Ok(n) => flags.context_lines = n,
                Err(_) => return arg_error(&format!("非法 -U 值: {a}")),
            },
            "--patch-style" => match next(&mut i).as_deref() {
                Some("unified") => flags.patch_style = PatchStyle::Unified,
                Some("context") => flags.patch_style = PatchStyle::Context,
                Some("normal") => flags.patch_style = PatchStyle::Normal,
                _ => return arg_error("--patch-style 仅支持 unified|context|normal"),
            },
            _ if next_or_eq(a, "--patch-style=").is_some() => {
                let v = next_or_eq(a, "--patch-style=").unwrap();
                flags.patch_style = match v.as_str() {
                    "unified" => PatchStyle::Unified,
                    "context" => PatchStyle::Context,
                    "normal" => PatchStyle::Normal,
                    _ => return arg_error("--patch-style 仅支持 unified|context|normal"),
                };
            }
            "--tsv" => flags.tsv = true,
            "--color" => flags.color = Some(ColorMode::Color),
            "--mono" => flags.color = Some(ColorMode::Mono),
            "--wrap" => match next(&mut i).as_deref() {
                Some("none") => flags.wrap = WrapMode::None,
                Some("character") => flags.wrap = WrapMode::Character,
                Some("word") => flags.wrap = WrapMode::Word,
                _ => return arg_error("--wrap 仅支持 none|character|word"),
            },
            _ if next_or_eq(a, "--wrap=").is_some() => {
                let v = next_or_eq(a, "--wrap=").unwrap();
                flags.wrap = match v.as_str() {
                    "none" => WrapMode::None,
                    "character" => WrapMode::Character,
                    "word" => WrapMode::Word,
                    _ => return arg_error("--wrap 仅支持 none|character|word"),
                };
            }
            "--wrap-columns" => match next(&mut i).and_then(|v| v.parse::<usize>().ok()) {
                Some(n) if n > 0 => flags.wrap_columns = n,
                _ => return arg_error("--wrap-columns 需要正整数"),
            },
            "--columns" => match next(&mut i) {
                Some(spec) => parse_columns(&spec, &mut flags),
                None => return arg_error("--columns 需要逗号分隔列名（size,timestamp,attributes,...）"),
            },
            _ if next_or_eq(a, "--columns=").is_some() => {
                let spec = next_or_eq(a, "--columns=").unwrap();
                parse_columns(&spec, &mut flags);
            }
            "--file-links" => flags.include_file_links = true,
            "--kind" => match next(&mut i).as_deref() {
                Some(k @ ("auto" | "dir" | "text" | "table" | "hex")) => flags.kind = Some(k.to_string()),
                _ => return arg_error("--kind 仅支持 auto|dir|text|table|hex"),
            },
            _ if next_or_eq(a, "--kind=").is_some() => {
                let v = next_or_eq(a, "--kind=").unwrap();
                match v.as_str() {
                    "auto" | "dir" | "text" | "table" | "hex" => flags.kind = Some(v),
                    _ => return arg_error("--kind 仅支持 auto|dir|text|table|hex"),
                }
            }
            "--rules" => match next(&mut i) {
                Some(p) => flags.rules_path = Some(p),
                None => return arg_error("--rules 需要规则文档路径"),
            },
            _ if next_or_eq(a, "--rules=").is_some() => {
                flags.rules_path = Some(next_or_eq(a, "--rules=").unwrap());
            }
            "--format-load" => match next(&mut i) {
                Some(p) => flags.format_pack_path = Some(p),
                None => return arg_error("--format-load 需要格式集路径"),
            },
            _ if next_or_eq(a, "--format-load=").is_some() => {
                flags.format_pack_path = Some(next_or_eq(a, "--format-load=").unwrap());
            }
            _ if a.starts_with('-') => return arg_error(&format!("未知参数: {a}")),
            _ => positional.push(&args[i]),
        }
        i += 1;
    }
    if positional.len() != 2 {
        return arg_error("report 需要恰好两个路径参数: <left> <right>");
    }
    let Some(layout) = flags.layout else {
        return arg_error("report 需要 --layout <布局名>");
    };

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio 运行时");
    match rt.block_on(run_report(positional[0], positional[1], layout, &flags)) {
        Ok(()) => EXIT_SAME.into(),
        Err(msg) => {
            eprintln!("aic report: {msg}");
            EXIT_ERROR.into()
        }
    }
}

fn parse_columns(spec: &str, flags: &mut ReportFlags) {
    // column-none 清除全部（FR-RPT-002）
    if spec.trim() == "none" {
        flags.columns = DirColumns {
            size: false,
            timestamp: false,
            attributes: false,
            crc: false,
            vcs: false,
            revision: false,
            version: false,
            owner: false,
            group: false,
        };
        return;
    }
    for part in spec.split(',') {
        match part.trim() {
            "size" => flags.columns.size = true,
            "timestamp" => flags.columns.timestamp = true,
            "attributes" => flags.columns.attributes = true,
            "crc" => flags.columns.crc = true,
            "vcs" => flags.columns.vcs = true,
            "revision" => flags.columns.revision = true,
            "version" => flags.columns.version = true,
            "owner" => flags.columns.owner = true,
            "group" => flags.columns.group = true,
            _ => {}
        }
    }
}

async fn run_report(
    left_s: &str,
    right_s: &str,
    layout: ReportLayout,
    flags: &ReportFlags,
) -> Result<(), String> {
    let vfs = RouteFs::new();
    let left_path = VfsPath::parse(left_s).map_err(|e| e.to_string())?;
    let right_path = VfsPath::parse(right_s).map_err(|e| e.to_string())?;
    let ls = vfs.stat(&left_path).await.map_err(|e| e.to_string())?;
    let rs = vfs.stat(&right_path).await.map_err(|e| e.to_string())?;

    // --kind 缺省 = auto（目录对 -> 文件夹报告，否则文本报告）
    let kind = flags.kind.as_deref().unwrap_or("auto");
    let kind_is = |k: &str| kind == k;
    let snap = if kind_is("table") {
        build_table_snapshot(&vfs, &left_path, &right_path, left_s, right_s, flags).await?
    } else if kind_is("hex") {
        build_hex_snapshot(&vfs, &left_path, &right_path, left_s, right_s).await?
    } else if kind_is("dir")
        || (kind_is("auto")
            && ls.kind == ai_vfs::EntryKind::Dir
            && rs.kind == ai_vfs::EntryKind::Dir)
    {
        build_dir_snapshot(&vfs, &left_path, &right_path, left_s, right_s, flags).await?
    } else {
        build_text_snapshot(&vfs, &left_path, &right_path, left_s, right_s, flags).await?
    };

    let opts = ReportOptions {
        title: flags.title.clone(),
        layout,
        display: flags.display,
        dir_display: flags.dir_display,
        ignore_unimportant: flags.ignore_unimportant,
        line_numbers: flags.line_numbers,
        context_lines: flags.context_lines,
        patch_style: flags.patch_style,
        delimiter: if flags.tsv { '\t' } else { ',' },
        color: flags.color.unwrap_or_default(),
        wrap: flags.wrap,
        wrap_columns: flags.wrap_columns,
        columns: flags.columns,
        include_file_links: flags.include_file_links,
    };
    let rendered = ai_core::report::render_report(&snap, &opts).map_err(|e| e.to_string())?;

    // 输出：-o 支持文件名模板（%f/%t 等）
    let naming = ReportNaming {
        now_ms: now_epoch_ms(),
        left_name: base_name(left_s),
        right_name: base_name(right_s),
        kind: snap.kind().to_string(),
    };
    match &flags.out {
        Some(tpl) => {
            let path = ai_core::expand_template(tpl, &naming);
            let ext = ai_core::layout_extension(layout);
            let path = if Path::new(&path).extension().is_none() {
                format!("{path}.{ext}")
            } else {
                path
            };
            std::fs::write(&path, rendered.as_bytes())
                .map_err(|e| format!("写报告失败 {path}: {e}"))?;
            println!("{path}");
        }
        None => {
            let stdout = std::io::stdout();
            let mut lock = stdout.lock();
            let _ = lock.write_all(rendered.as_bytes());
            let _ = lock.flush();
        }
    }
    Ok(())
}

fn base_name(p: &str) -> String {
    Path::new(p)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(p)
        .to_string()
}

pub(crate) fn now_epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

async fn build_text_snapshot(
    vfs: &RouteFs,
    left: &VfsPath,
    right: &VfsPath,
    left_s: &str,
    right_s: &str,
    flags: &ReportFlags,
) -> Result<ReportSnapshot, String> {
    let lt_text = vfs.read_file_text(left).await.map_err(|e| e.to_string())?;
    let rt_text = vfs.read_file_text(right).await.map_err(|e| e.to_string())?;
    let lt = ai_core::LineTable::from_text(&lt_text);
    let rt = ai_core::LineTable::from_text(&rt_text);

    let mut config = ai_core::DiffConfig::default();
    // 重要性开关（--ignore-unimportant：空白三类 + 大小写不重要）
    if flags.ignore_unimportant {
        config.importance.leading_ws_unimportant = true;
        config.importance.embedded_ws_unimportant = true;
        config.importance.trailing_ws_unimportant = true;
        config.importance.case_unimportant = true;
    }
    // P10 规则/格式集（与 aic diff 同语义：加载即生效）
    let mut registry = ai_core::FormatRegistry::builtin();
    if let Some(pack_path) = &flags.format_pack_path {
        let text = std::fs::read_to_string(pack_path)
            .map_err(|e| format!("格式集读取失败 {pack_path}: {e}"))?;
        let pack =
            ai_core::FormatPack::from_json(&text).map_err(|e| format!("格式集解析失败: {e}"))?;
        registry.merge_pack(&pack);
    }
    if let Some(rules_path) = &flags.rules_path {
        let text = std::fs::read_to_string(rules_path)
            .map_err(|e| format!("规则文档读取失败 {rules_path}: {e}"))?;
        let doc = ai_core::RulesDocument::from_json(&text)
            .map_err(|e| format!("规则文档解析失败: {e}"))?;
        let set = ai_core::RuleSet::from_document(&doc);
        let fname = base_name(left_s);
        let head = vfs.read_range(left, 0, 8192).await.unwrap_or_default();
        let fmt = registry.detect(&fname, &head);
        let effective = set.resolve(None, None, fmt.map(|f| f.id.as_str()));
        config.importance = effective.to_importance_options();
        config.base.rules = ai_core::RulesOptions::resolve(&effective, fmt);
    }

    let keyed = ai_core::diff_config(&lt, &rt, &config).map_err(|e| e.to_string())?;
    Ok(ReportSnapshot::Text(TextSnapshot::from_tables(
        left_s,
        right_s,
        &lt,
        &rt,
        keyed.output.sections,
    )))
}

async fn build_dir_snapshot(
    vfs: &RouteFs,
    left: &VfsPath,
    right: &VfsPath,
    left_s: &str,
    right_s: &str,
    flags: &ReportFlags,
) -> Result<ReportSnapshot, String> {
    let mut criteria = CompareCriteria {
        mtime_tolerance_sec: 0,
        content_compare: ai_protocol::ContentCompareMode::Always,
        ..Default::default()
    };
    if flags.ignore_unimportant {
        criteria.compare_case = false;
    }
    let result = ai_dir::scan(
        vfs,
        left,
        right,
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
    let rows = result
        .arena
        .iter()
        .skip(1)
        .map(dir_node_to_report_row)
        .collect();
    let stats = ai_core::DirReportStats {
        total: result.stats.total,
        same: result.stats.same,
        quick_diff: result.stats.quick_diff,
        content_diff: result.stats.content_diff,
        left_only: result.stats.left_only,
        right_only: result.stats.right_only,
        dir_same: result.stats.dir_same,
        dir_diff: result.stats.dir_diff,
    };
    Ok(ReportSnapshot::Dir(DirSnapshot {
        left_label: left_s.to_string(),
        right_label: right_s.to_string(),
        rows,
        stats,
    }))
}

/// ai-dir DirNode -> 报告行（与引擎 mapping 同构；CLI 侧独立映射避免依赖 ai-engine）
pub(crate) fn dir_node_to_report_row(n: &ai_dir::DirNode) -> ai_core::DirReportRow {
    let side = |s: &Option<ai_protocol::SideInfo>| {
        s.as_ref().map(|i| ReportSideInfo {
            size: i.size,
            mtime_ms: i.mtime_ms,
            attribute: i.attribute.clone(),
        })
    };
    ai_core::DirReportRow {
        rel_path: n.rel_path.clone(),
        name: n.name.clone(),
        kind: match n.kind {
            ai_protocol::EntryKind::File => ReportEntryKind::File,
            ai_protocol::EntryKind::Dir => ReportEntryKind::Dir,
            ai_protocol::EntryKind::Symlink => ReportEntryKind::Symlink,
        },
        status: match n.status {
            ai_protocol::DirEntryStatus::Same => ReportEntryStatus::Same,
            ai_protocol::DirEntryStatus::QuickDiff => ReportEntryStatus::QuickDiff,
            ai_protocol::DirEntryStatus::ContentDiff => ReportEntryStatus::ContentDiff,
            ai_protocol::DirEntryStatus::Similar => ReportEntryStatus::Similar,
            ai_protocol::DirEntryStatus::LeftOnly => ReportEntryStatus::LeftOnly,
            ai_protocol::DirEntryStatus::RightOnly => ReportEntryStatus::RightOnly,
            ai_protocol::DirEntryStatus::DirSame => ReportEntryStatus::DirSame,
            ai_protocol::DirEntryStatus::DirDiff => ReportEntryStatus::DirDiff,
            ai_protocol::DirEntryStatus::Unknown => ReportEntryStatus::Unknown,
        },
        left: side(&n.left),
        right: side(&n.right),
    }
}

async fn build_table_snapshot(
    vfs: &RouteFs,
    left: &VfsPath,
    right: &VfsPath,
    left_s: &str,
    right_s: &str,
    flags: &ReportFlags,
) -> Result<ReportSnapshot, String> {
    let lt_text = vfs.read_file_text(left).await.map_err(|e| e.to_string())?;
    let rt_text = vfs.read_file_text(right).await.map_err(|e| e.to_string())?;
    let mut opts = ai_core::table::TableDiffOptions::default();
    if flags.tsv {
        opts.parse.delimiter = '\t';
    }
    if flags.ignore_unimportant {
        opts.importance.case_unimportant = true;
        opts.importance.whitespace = ai_core::options::WhitespaceMode::IgnoreAmount;
    }
    let model = ai_core::table::diff_tables(&lt_text, &rt_text, &opts)
        .map_err(|e| format!("表格解析失败: {e}"))?;
    Ok(ReportSnapshot::Table(TableSnapshot {
        left_label: left_s.to_string(),
        right_label: right_s.to_string(),
        columns: model.columns,
        rows: model.rows,
        summary: model.summary,
        has_header: opts.parse.has_header,
    }))
}

async fn build_hex_snapshot(
    vfs: &RouteFs,
    left: &VfsPath,
    right: &VfsPath,
    left_s: &str,
    right_s: &str,
) -> Result<ReportSnapshot, String> {
    let lb = vfs.read_file_bytes(left).await.map_err(|e| e.to_string())?;
    let rb = vfs
        .read_file_bytes(right)
        .await
        .map_err(|e| e.to_string())?;
    let model = ai_core::hex::diff_bytes(&lb, &rb, &ai_core::hex::HexOptions::default());
    let (sl, sr) = ai_core::hex::status_maps(&model);
    Ok(ReportSnapshot::Hex(HexSnapshot {
        left_label: left_s.to_string(),
        right_label: right_s.to_string(),
        left: lb,
        right: rb,
        model,
        status_left: sl,
        status_right: sr,
    }))
}
