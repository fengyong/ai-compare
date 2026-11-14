//! 报告引擎（P11 / FR-RPT-001、FR-RPT-002 / FR-PATCH-010；docs/04 §P11）。
//!
//! L2 纯函数层：输入会话快照（[`ReportSnapshot`]，由上层从 DirModel /
//! TextSession / TableSession / HexSession / MergeSession 转换而来），
//! 输出报告字符串。无 IO、无时钟依赖（文件名模板的时间由调用方注入，
//! 保证同输入 -> 同输出，支撑 golden 快照测试）。
//!
//! 布局族（任务书 8 类 + FR-RPT-002 文本比较的 side-by-side / interleaved）：
//! ① [`ReportLayout::Summary`] 纯文本摘要（统计 + 差异清单）
//! ② [`ReportLayout::Patch`] 补丁布局（FR-PATCH-010：normal/context/unified 三式）
//! ③ [`ReportLayout::Html`] 自包含 HTML（样式内联 `<style>`，色板语义：
//!    红=删除/蓝=修改/绿=插入/灰=孤项，与 BC 对齐；color/mono 两态）
//! ④ [`ReportLayout::Xml`] 结构化 XML（机器消费）
//! ⑤ [`ReportLayout::Csv`] CSV/TSV 表格化（RFC4180 引号转义）
//! ⑥ [`ReportLayout::Tree`] 文件夹树形（缩进树 + 状态标记）
//! ⑦ [`ReportLayout::List`] 文件清单（平铺行式）
//! ⑧ [`ReportLayout::Stats`] 变更统计（只统计无内容）
//! ⑨ [`ReportLayout::SideBySide`] 双栏并排
//! ⑩ [`ReportLayout::Interleaved`] 全量交错（unified 式前缀、不裁上下文）
//!
//! 渲染器统一抽象：[`ReportRenderer`] trait + [`renderer_for_layout`]；
//! 一站式入口 [`render_report`]（按布局分发）。
//!
//! 文件名/时间戳模板（BC 风格）：[`expand_template`] 支持
//! `%f` 左基名 / `%F` 右基名 / `%k` 会话种类 / `%t` 完整时间戳
//! （yyyy-mm-dd_hh-mm-ss，文件名安全）/ `%T` 日期（yyyy-mm-dd）/
//! `%Y %m %d %H %M %S` 分量 / `%%` 字面 %。

use crate::hex::{HexBlockKind, HexByteStatus, HexDiffModel, HEX_LINE_BYTES};
use crate::merge::{ChunkOrigin, MergeModel};
use crate::sections::{Importance, Section, SectionKind};
use crate::table::{ComparisonColumn, TableDiffRow, TableDiffSummary};

// ---------------------------------------------------------------------------
// 会话快照模型（上层转换；本层只读）
// ---------------------------------------------------------------------------

/// 条目种类（协议 EntryKind 的报告侧镜像）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportEntryKind {
    File,
    Dir,
    Symlink,
}

impl ReportEntryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ReportEntryKind::File => "file",
            ReportEntryKind::Dir => "dir",
            ReportEntryKind::Symlink => "symlink",
        }
    }
}

/// 条目状态（协议 DirEntryStatus 的报告侧镜像，字符串同形）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportEntryStatus {
    Same,
    QuickDiff,
    ContentDiff,
    LeftOnly,
    RightOnly,
    DirSame,
    DirDiff,
    Unknown,
}

impl ReportEntryStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ReportEntryStatus::Same => "same",
            ReportEntryStatus::QuickDiff => "quickDiff",
            ReportEntryStatus::ContentDiff => "contentDiff",
            ReportEntryStatus::LeftOnly => "leftOnly",
            ReportEntryStatus::RightOnly => "rightOnly",
            ReportEntryStatus::DirSame => "dirSame",
            ReportEntryStatus::DirDiff => "dirDiff",
            ReportEntryStatus::Unknown => "unknown",
        }
    }
}

/// 差异类状态（内容差异 / 快速差异 / 目录聚合差异 / 孤项）
pub fn status_is_diff(s: ReportEntryStatus) -> bool {
    matches!(
        s,
        ReportEntryStatus::QuickDiff | ReportEntryStatus::ContentDiff | ReportEntryStatus::DirDiff
    )
}

/// 孤项类状态（单侧存在）
pub fn status_is_orphan(s: ReportEntryStatus) -> bool {
    matches!(s, ReportEntryStatus::LeftOnly | ReportEntryStatus::RightOnly)
}

/// 单侧元数据（报告用；mtime 为 epoch 毫秒）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportSideInfo {
    pub size: u64,
    pub mtime_ms: u64,
    /// DOS 属性字母串（可选列 attributes）
    pub attribute: Option<String>,
}

impl ReportSideInfo {
    pub fn new(size: u64, mtime_ms: u64) -> Self {
        ReportSideInfo { size, mtime_ms, attribute: None }
    }
}

/// 文件夹报告行（rel_path 为 POSIX 相对路径；目录行尾随 `/`）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirReportRow {
    pub rel_path: String,
    pub name: String,
    pub kind: ReportEntryKind,
    pub status: ReportEntryStatus,
    pub left: Option<ReportSideInfo>,
    pub right: Option<ReportSideInfo>,
}

impl DirReportRow {
    /// 显示路径（目录尾随 /，与 CLI 目录 diff 约定一致）
    pub fn display_path(&self) -> String {
        if self.kind == ReportEntryKind::Dir {
            format!("{}/", self.rel_path)
        } else {
            self.rel_path.clone()
        }
    }
    /// 深度（路径段数-1；顶层 = 0）
    pub fn depth(&self) -> usize {
        self.rel_path.split('/').filter(|s| !s.is_empty()).count().saturating_sub(1)
    }
}

/// 文件夹统计
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DirReportStats {
    pub total: u32,
    pub same: u32,
    pub quick_diff: u32,
    pub content_diff: u32,
    pub left_only: u32,
    pub right_only: u32,
    pub dir_same: u32,
    pub dir_diff: u32,
}

/// 文本比较快照（sections 与行表自洽：区间索引指向行数组）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextSnapshot {
    pub left_label: String,
    pub right_label: String,
    pub left_lines: Vec<String>,
    pub right_lines: Vec<String>,
    pub sections: Vec<Section>,
    /// 非重要 section 数（ignore-unimportant 统计口径）
    pub unimportant_sections: u32,
    /// 左侧末行无换行符（patch 布局输出 `\ No newline` 标记用）
    pub left_no_trailing_newline: bool,
    /// 右侧末行无换行符
    pub right_no_trailing_newline: bool,
}

impl TextSnapshot {
    /// 从 sections 构建快照（行数组由调用方提供；unimportant 计数自动统计；
    /// 行尾标志缺省 false——patch 布局需要精确标记时用 [`TextSnapshot::from_tables`]）
    pub fn new(
        left_label: impl Into<String>,
        right_label: impl Into<String>,
        left_lines: Vec<String>,
        right_lines: Vec<String>,
        sections: Vec<Section>,
    ) -> Self {
        let unimportant_sections = sections
            .iter()
            .filter(|s| s.kind != SectionKind::Equal && s.important != Importance::Important)
            .count() as u32;
        TextSnapshot {
            left_label: left_label.into(),
            right_label: right_label.into(),
            left_lines,
            right_lines,
            sections,
            unimportant_sections,
            left_no_trailing_newline: false,
            right_no_trailing_newline: false,
        }
    }

    /// 从两侧行表构建（含末行无换行标志；unified patch 精确还原所需）
    pub fn from_tables(
        left_label: impl Into<String>,
        right_label: impl Into<String>,
        left: &crate::LineTable,
        right: &crate::LineTable,
        sections: Vec<Section>,
    ) -> Self {
        let no_nl = |t: &crate::LineTable| {
            !t.is_empty() && t.line(t.len() - 1).ending == crate::lines::LineEnding::None
        };
        let mut snap = TextSnapshot::new(
            left_label,
            right_label,
            (0..left.len()).map(|i| left.line(i).text.clone()).collect(),
            (0..right.len()).map(|i| right.line(i).text.clone()).collect(),
            sections,
        );
        snap.left_no_trailing_newline = no_nl(left);
        snap.right_no_trailing_newline = no_nl(right);
        snap
    }

    pub fn has_differences(&self) -> bool {
        self.sections.iter().any(|s| s.kind != SectionKind::Equal)
    }
}

/// 文件夹比较快照
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirSnapshot {
    pub left_label: String,
    pub right_label: String,
    pub rows: Vec<DirReportRow>,
    pub stats: DirReportStats,
}

/// 表格比较快照（复用 P9 表格引擎模型）
#[derive(Debug, Clone, PartialEq)]
pub struct TableSnapshot {
    pub left_label: String,
    pub right_label: String,
    pub columns: Vec<ComparisonColumn>,
    pub rows: Vec<TableDiffRow>,
    pub summary: TableDiffSummary,
    /// 首行为表头（CSV 输出列名用）
    pub has_header: bool,
}

/// 十六进制比较快照（字节流驻快照，渲染 hex dump 用）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HexSnapshot {
    pub left_label: String,
    pub right_label: String,
    pub left: Vec<u8>,
    pub right: Vec<u8>,
    pub model: HexDiffModel,
    /// 左侧每字节状态（同长 = left.len()；模型自洽）
    pub status_left: Vec<HexByteStatus>,
    pub status_right: Vec<HexByteStatus>,
}

/// 三向合并快照
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeSnapshot {
    pub base_label: String,
    pub left_label: String,
    pub right_label: String,
    pub model: MergeModel,
    /// 未决冲突块 id
    pub unresolved: Vec<u32>,
}

/// 会话快照（报告输入的统一形态）
#[derive(Debug, Clone, PartialEq)]
pub enum ReportSnapshot {
    Text(TextSnapshot),
    Dir(DirSnapshot),
    Table(TableSnapshot),
    Hex(HexSnapshot),
    Merge(MergeSnapshot),
}

impl ReportSnapshot {
    /// 会话种类名（标题/模板 %k 占位符用）
    pub fn kind(&self) -> &'static str {
        match self {
            ReportSnapshot::Text(_) => "text",
            ReportSnapshot::Dir(_) => "dir",
            ReportSnapshot::Table(_) => "table",
            ReportSnapshot::Hex(_) => "hex",
            ReportSnapshot::Merge(_) => "merge",
        }
    }

    /// 是否存在任何差异（各快照口径）
    pub fn has_differences(&self) -> bool {
        match self {
            ReportSnapshot::Text(s) => s.has_differences(),
            ReportSnapshot::Dir(s) => s.stats.content_diff > 0
                || s.stats.quick_diff > 0
                || s.stats.left_only > 0
                || s.stats.right_only > 0
                || s.stats.dir_diff > 0,
            ReportSnapshot::Table(s) => {
                s.summary.changed_rows > 0 || s.summary.added_rows > 0 || s.summary.removed_rows > 0
            }
            ReportSnapshot::Hex(s) => {
                s.model.summary.changed_bytes > 0
                    || s.model.summary.inserted_bytes > 0
                    || s.model.summary.deleted_bytes > 0
            }
            ReportSnapshot::Merge(s) => s.model.conflict_count() > 0
                || s.model.chunks.iter().any(|c| {
                    matches!(c.origin, ChunkOrigin::LeftOnly | ChunkOrigin::RightOnly | ChunkOrigin::BothChanged)
                }),
        }
    }
}

// ---------------------------------------------------------------------------
// 布局与选项
// ---------------------------------------------------------------------------

/// 报告布局（FR-RPT-002 布局族）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReportLayout {
    /// 纯文本摘要：统计 + 差异清单
    Summary,
    /// 双栏并排
    SideBySide,
    /// 全量交错（unified 前缀、不裁上下文；FR-RPT-002 interleaved）
    Interleaved,
    /// 补丁（FR-PATCH-010：patch-normal/context/unified）
    Patch,
    /// 自包含 HTML
    Html,
    /// 结构化 XML
    Xml,
    /// CSV/TSV 表格化
    Csv,
    /// 文件夹缩进树 + 状态标记
    Tree,
    /// 文件清单
    List,
    /// 变更统计（只统计无内容）
    Stats,
}

impl ReportLayout {
    pub fn as_str(self) -> &'static str {
        match self {
            ReportLayout::Summary => "summary",
            ReportLayout::SideBySide => "side-by-side",
            ReportLayout::Interleaved => "interleaved",
            ReportLayout::Patch => "patch",
            ReportLayout::Html => "html",
            ReportLayout::Xml => "xml",
            ReportLayout::Csv => "csv",
            ReportLayout::Tree => "tree",
            ReportLayout::List => "list",
            ReportLayout::Stats => "stats",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "summary" => Some(ReportLayout::Summary),
            "side-by-side" | "sidebyside" | "sidebyside2" => Some(ReportLayout::SideBySide),
            "interleaved" => Some(ReportLayout::Interleaved),
            "patch" => Some(ReportLayout::Patch),
            "html" => Some(ReportLayout::Html),
            "xml" => Some(ReportLayout::Xml),
            "csv" | "tsv" => Some(ReportLayout::Csv),
            "tree" => Some(ReportLayout::Tree),
            "list" => Some(ReportLayout::List),
            "stats" | "statistics" => Some(ReportLayout::Stats),
            _ => None,
        }
    }
}

/// 文本差异显示过滤（FR-RPT-002 display-all/mismatches/context/matches）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DisplayFilter {
    #[default]
    All,
    /// 只显示差异 section
    Mismatches,
    /// 差异 section + 前后 context_lines 条相同行
    Context,
    /// 只显示相同 section
    Matches,
}

/// 文件夹显示过滤（FR-RPT-002 文件夹 display 标志集）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DirDisplayFilter {
    #[default]
    All,
    Mismatches,
    NoOrphans,
    MismatchesNoOrphans,
    Orphans,
    Matches,
    LeftNewer,
    RightNewer,
    LeftNewerOrphans,
    RightNewerOrphans,
    LeftOrphans,
    RightOrphans,
}

impl DirDisplayFilter {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "all" => Some(DirDisplayFilter::All),
            "mismatches" => Some(DirDisplayFilter::Mismatches),
            "no-orphans" => Some(DirDisplayFilter::NoOrphans),
            "mismatches-no-orphans" => Some(DirDisplayFilter::MismatchesNoOrphans),
            "orphans" => Some(DirDisplayFilter::Orphans),
            "matches" => Some(DirDisplayFilter::Matches),
            "left-newer" => Some(DirDisplayFilter::LeftNewer),
            "right-newer" => Some(DirDisplayFilter::RightNewer),
            "left-newer-orphans" => Some(DirDisplayFilter::LeftNewerOrphans),
            "right-newer-orphans" => Some(DirDisplayFilter::RightNewerOrphans),
            "left-orphans" => Some(DirDisplayFilter::LeftOrphans),
            "right-orphans" => Some(DirDisplayFilter::RightOrphans),
            _ => None,
        }
    }

    fn keeps(&self, row: &DirReportRow) -> bool {
        let diff = status_is_diff(row.status);
        let orphan = status_is_orphan(row.status);
        let newer = |side_l: bool| match (&row.left, &row.right) {
            (Some(l), Some(r)) if l.mtime_ms != r.mtime_ms => {
                (l.mtime_ms > r.mtime_ms) == side_l
            }
            _ => false,
        };
        match self {
            DirDisplayFilter::All => true,
            DirDisplayFilter::Mismatches => diff || orphan,
            DirDisplayFilter::NoOrphans => !orphan,
            DirDisplayFilter::MismatchesNoOrphans => diff,
            DirDisplayFilter::Orphans => orphan,
            DirDisplayFilter::Matches => !diff && !orphan,
            DirDisplayFilter::LeftNewer => newer(true),
            DirDisplayFilter::RightNewer => newer(false),
            DirDisplayFilter::LeftNewerOrphans => newer(true) || matches!(row.status, ReportEntryStatus::LeftOnly),
            DirDisplayFilter::RightNewerOrphans => newer(false) || matches!(row.status, ReportEntryStatus::RightOnly),
            DirDisplayFilter::LeftOrphans => matches!(row.status, ReportEntryStatus::LeftOnly),
            DirDisplayFilter::RightOrphans => matches!(row.status, ReportEntryStatus::RightOnly),
        }
    }
}

/// 补丁格式（FR-PATCH-010）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PatchStyle {
    /// 标准 diff（normal）
    Normal,
    /// 上下文 diff
    Context,
    /// 统一 diff（缺省）
    #[default]
    Unified,
}

/// 颜色模式（FR-RPT-001 print/html color|mono）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColorMode {
    #[default]
    Color,
    Mono,
}

/// 换行模式（FR-RPT-001 wrap-none/character/word；HTML 仅 none/word）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WrapMode {
    #[default]
    None,
    /// 按字符硬折行（配合 wrap_columns）
    Character,
    /// HTML word-wrap（CSS 语义）
    Word,
}

/// 文件夹可选列（FR-RPT-002 column-*；默认 size+timestamp）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirColumns {
    pub size: bool,
    pub timestamp: bool,
    pub attributes: bool,
    pub crc: bool,
    pub vcs: bool,
    pub revision: bool,
    pub version: bool,
    pub owner: bool,
    pub group: bool,
}

impl Default for DirColumns {
    fn default() -> Self {
        DirColumns {
            size: true,
            timestamp: true,
            attributes: false,
            crc: false,
            vcs: false,
            revision: false,
            version: false,
            owner: false,
            group: false,
        }
    }
}

/// 报告渲染选项（跨布局公共面；FR-RPT-001 报告对话框选项）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportOptions {
    /// 报告标题（None = 自动生成 "<kind> compare report"）
    pub title: Option<String>,
    pub layout: ReportLayout,
    /// 文本差异显示过滤
    pub display: DisplayFilter,
    /// 文件夹显示过滤
    pub dir_display: DirDisplayFilter,
    /// 忽略不重要差异（不重要 section 不计入差异清单）
    pub ignore_unimportant: bool,
    /// 显示行号（side-by-side 行号 / hex 字节地址）
    pub line_numbers: bool,
    /// 上下文行数（patch 与 display=context 共用；默认 3）
    pub context_lines: u32,
    pub patch_style: PatchStyle,
    /// CSV 分隔符（','；TSV 传 '\t'）
    pub delimiter: char,
    pub color: ColorMode,
    pub wrap: WrapMode,
    /// 字符硬折行列数（wrap=Character 时生效）
    pub wrap_columns: usize,
    pub columns: DirColumns,
    /// 文件夹 side-by-side HTML 内嵌逐文件报告链接（FR-RPT-002 include-file-links）
    pub include_file_links: bool,
}

impl Default for ReportOptions {
    fn default() -> Self {
        ReportOptions {
            title: None,
            layout: ReportLayout::Summary,
            display: DisplayFilter::All,
            dir_display: DirDisplayFilter::All,
            ignore_unimportant: false,
            line_numbers: false,
            context_lines: DEFAULT_REPORT_CONTEXT,
            patch_style: PatchStyle::Unified,
            delimiter: ',',
            color: ColorMode::Color,
            wrap: WrapMode::None,
            wrap_columns: 80,
            columns: DirColumns::default(),
            include_file_links: false,
        }
    }
}

/// patch / context 显示的默认上下文行数
pub const DEFAULT_REPORT_CONTEXT: u32 = 3;

/// 渲染错误
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReportError {
    #[error("布局 {layout} 不支持 {kind} 会话快照")]
    Unsupported { layout: &'static str, kind: &'static str },
    #[error("报告渲染失败: {0}")]
    Render(String),
}

// ---------------------------------------------------------------------------
// 渲染器 trait 与分发
// ---------------------------------------------------------------------------

/// 报告渲染器（统一抽象：输入会话快照 + 选项，输出字符串）
pub trait ReportRenderer: Send + Sync {
    fn layout(&self) -> ReportLayout;
    fn render(&self, snap: &ReportSnapshot, opts: &ReportOptions) -> Result<String, ReportError>;
}

/// 按布局取渲染器
pub fn renderer_for_layout(layout: ReportLayout) -> Box<dyn ReportRenderer> {
    match layout {
        ReportLayout::Summary => Box::new(SummaryRenderer),
        ReportLayout::SideBySide => Box::new(SideBySideRenderer),
        ReportLayout::Interleaved => Box::new(InterleavedRenderer),
        ReportLayout::Patch => Box::new(PatchRenderer),
        ReportLayout::Html => Box::new(HtmlRenderer),
        ReportLayout::Xml => Box::new(XmlRenderer),
        ReportLayout::Csv => Box::new(CsvRenderer),
        ReportLayout::Tree => Box::new(TreeRenderer),
        ReportLayout::List => Box::new(ListRenderer),
        ReportLayout::Stats => Box::new(StatsRenderer),
    }
}

/// 一站式渲染入口（按 opts.layout 分发）
pub fn render_report(snap: &ReportSnapshot, opts: &ReportOptions) -> Result<String, ReportError> {
    renderer_for_layout(opts.layout).render(snap, opts)
}

fn unsupported(layout: ReportLayout, snap: &ReportSnapshot) -> ReportError {
    ReportError::Unsupported { layout: layout.as_str(), kind: snap.kind() }
}

fn effective_title(snap: &ReportSnapshot, opts: &ReportOptions) -> String {
    opts.title
        .clone()
        .unwrap_or_else(|| format!("{} compare report", snap.kind()))
}

// ---------------------------------------------------------------------------
// section 可见性（display 过滤 + ignore-unimportant）
// ---------------------------------------------------------------------------

enum SectionVis {
    /// 差异（参与差异清单）
    Diff,
    /// 相同上下文（Matches/All 显示）
    Same,
    Skip,
}

fn section_visibility(sec: &Section, opts: &ReportOptions) -> SectionVis {
    let is_diff_kind = sec.kind != SectionKind::Equal;
    let unimportant = is_diff_kind && sec.important != Importance::Important;
    if opts.ignore_unimportant && unimportant {
        return SectionVis::Skip;
    }
    if !is_diff_kind {
        return match opts.display {
            DisplayFilter::All => SectionVis::Same,
            DisplayFilter::Mismatches => SectionVis::Skip,
            DisplayFilter::Context => SectionVis::Same, // 邻近裁剪在调用方做
            DisplayFilter::Matches => SectionVis::Same,
        };
    }
    match opts.display {
        DisplayFilter::Matches => SectionVis::Skip,
        _ => SectionVis::Diff,
    }
}

/// context 显示模式下的 equal section 裁剪：
/// equal section 与最近差异 section 的行距 <= context_lines 才保留。
fn apply_context_prune<'a>(sections: &[&'a Section], opts: &ReportOptions) -> Vec<&'a Section> {
    if opts.display != DisplayFilter::Context {
        return sections.to_vec();
    }
    let ctx = opts.context_lines as i64;
    // 差异 section 判定（ignore-unimportant 下不重要差异视作相同）
    let diff_at = |i: usize| -> bool {
        sections[i].kind != SectionKind::Equal
            && !(opts.ignore_unimportant && sections[i].important != Importance::Important)
    };
    let mut keep: Vec<bool> = Vec::with_capacity(sections.len());
    for (i, s) in sections.iter().enumerate() {
        if s.kind != SectionKind::Equal {
            keep.push(diff_at(i));
            continue;
        }
        // 找前后最近的差异 section 的行距
        let mut near = i64::MAX;
        let l = s.left.map(|r| r.start as i64).unwrap_or(0);
        // 向前
        let mut j = i;
        while j > 0 {
            j -= 1;
            if diff_at(j) {
                let d = l - sections[j].left.map(|r| r.end as i64).unwrap_or(0);
                near = near.min(d);
                break;
            }
        }
        // 向后
        let mut j = i;
        while j + 1 < sections.len() {
            j += 1;
            if diff_at(j) {
                let d = sections[j].left.map(|r| r.start as i64).unwrap_or(0) - l;
                near = near.min(d);
                break;
            }
        }
        keep.push(near <= ctx);
    }
    sections
        .iter()
        .zip(keep)
        .filter(|(_, k)| *k)
        .map(|(s, _)| *s)
        .collect()
}

/// 取可见 section 序列（display 过滤 + context 裁剪 + ignore-unimportant）
fn visible_text_sections<'a>(snap: &'a TextSnapshot, opts: &ReportOptions) -> Vec<&'a Section> {
    let all: Vec<&Section> = snap.sections.iter().collect();
    let kept: Vec<&Section> = all
        .into_iter()
        .filter(|s| !matches!(section_visibility(s, opts), SectionVis::Skip))
        .collect();
    apply_context_prune(&kept, opts)
}

fn lines_of<'a>(
    snap: &'a TextSnapshot,
    side: crate::options::DiffSide,
    sec: &Section,
) -> &'a [String] {
    let range = match side {
        crate::options::DiffSide::Left => sec.left,
        crate::options::DiffSide::Right => sec.right,
    };
    match range {
        Some(r) => {
            let table = match side {
                crate::options::DiffSide::Left => &snap.left_lines,
                crate::options::DiffSide::Right => &snap.right_lines,
            };
            let s = (r.start as usize).min(table.len());
            let e = (r.end as usize).min(table.len());
            &table[s..e]
        }
        None => &[],
    }
}

// ---------------------------------------------------------------------------
// ① Summary：统计 + 差异清单
// ---------------------------------------------------------------------------

pub struct SummaryRenderer;

impl ReportRenderer for SummaryRenderer {
    fn layout(&self) -> ReportLayout {
        ReportLayout::Summary
    }

    fn render(&self, snap: &ReportSnapshot, opts: &ReportOptions) -> Result<String, ReportError> {
        let mut out = String::new();
        out.push_str(&effective_title(snap, opts));
        out.push('\n');
        match snap {
            ReportSnapshot::Text(s) => render_text_summary(&mut out, s, opts),
            ReportSnapshot::Dir(s) => render_dir_summary(&mut out, s, opts),
            ReportSnapshot::Table(s) => {
                out.push_str(&format!(
                    "左: {}  右: {}\n统计: same={} changed={} added={} removed={} unimportant={}\n",
                    s.left_label,
                    s.right_label,
                    s.summary.same_rows,
                    s.summary.changed_rows,
                    s.summary.added_rows,
                    s.summary.removed_rows,
                    s.summary.unimportant_rows
                ));
                if opts.display != DisplayFilter::Matches {
                    for row in &s.rows {
                        if row.kind == crate::table::TableRowKind::Same {
                            continue;
                        }
                        out.push_str(&format!(
                            "  [{row_kind}] 行 L{left} R{right}\n",
                            row_kind = row.kind.as_str(),
                            left = row.left.map(|v| v.to_string()).unwrap_or_else(|| "-".into()),
                            right = row.right.map(|v| v.to_string()).unwrap_or_else(|| "-".into()),
                        ));
                    }
                }
            }
            ReportSnapshot::Hex(s) => {
                out.push_str(&format!(
                    "左: {} ({} 字节)  右: {} ({} 字节)\n统计: same={}B changed={}B inserted={}B deleted={}B\n",
                    s.left_label,
                    s.model.left_len,
                    s.right_label,
                    s.model.right_len,
                    s.model.summary.same_bytes,
                    s.model.summary.changed_bytes,
                    s.model.summary.inserted_bytes,
                    s.model.summary.deleted_bytes,
                ));
                for b in &s.model.blocks {
                    if b.kind != HexBlockKind::Same {
                        out.push_str(&format!("  [{}]\n", hex_block_kind_str(b.kind)));
                    }
                }
            }
            ReportSnapshot::Merge(s) => {
                out.push_str(&format!(
                    "基: {}  左: {}  右: {}\n统计: chunks={} conflicts={} unresolved={}\n",
                    s.base_label,
                    s.left_label,
                    s.right_label,
                    s.model.chunks.len(),
                    s.model.conflict_count(),
                    s.unresolved.len()
                ));
                for c in &s.model.chunks {
                    if c.origin != ChunkOrigin::Stable {
                        out.push_str(&format!(
                            "  [{}] chunk#{} base={}-{} left={}-{} right={}-{}\n",
                            merge_origin_str(c.origin),
                            c.id,
                            c.base.start,
                            c.base.end,
                            c.left.start,
                            c.left.end,
                            c.right.start,
                            c.right.end
                        ));
                    }
                }
            }
        }
        Ok(out)
    }
}

fn render_text_summary(out: &mut String, s: &TextSnapshot, opts: &ReportOptions) {
    out.push_str(&format!("左: {}  右: {}\n", s.left_label, s.right_label));
    // 统计（与 sections 一致的重算口径）
    let mut equal_lines = 0u32;
    let mut inserted = 0u32;
    let mut deleted = 0u32;
    let mut diff_sections = 0u32;
    for sec in &s.sections {
        match sec.kind {
            SectionKind::Equal => equal_lines += sec.left.map_or(0, |r| r.len()),
            SectionKind::Insert => {
                inserted += sec.right.map_or(0, |r| r.len());
                diff_sections += 1;
            }
            SectionKind::Delete => {
                deleted += sec.left.map_or(0, |r| r.len());
                diff_sections += 1;
            }
            SectionKind::Change => {
                deleted += sec.left.map_or(0, |r| r.len());
                inserted += sec.right.map_or(0, |r| r.len());
                diff_sections += 1;
            }
        }
    }
    out.push_str(&format!(
        "统计: 差异块={diff_sections} 插入行={inserted} 删除行={deleted} 相同行={equal_lines} 不重要块={}\n",
        s.unimportant_sections
    ));
    if opts.display == DisplayFilter::Matches {
        return;
    }
    out.push_str("差异清单:\n");
    let vis = visible_text_sections(s, opts);
    for sec in vis {
        if sec.kind == SectionKind::Equal {
            continue;
        }
        let l = sec.left.map(|r| format!("{}-{}", r.start + 1, r.end));
        let r = sec.right.map(|r| format!("{}-{}", r.start + 1, r.end));
        out.push_str(&format!(
            "  [{kind}] 左 {l} / 右 {r}\n",
            kind = section_kind_str(sec.kind),
            l = l.unwrap_or_else(|| "-".into()),
            r = r.unwrap_or_else(|| "-".into()),
        ));
        // 首行预览（确定性：各侧第一行）
        let left_first = lines_of(s, crate::options::DiffSide::Left, sec).first().cloned();
        let right_first = lines_of(s, crate::options::DiffSide::Right, sec).first().cloned();
        if let Some(t) = left_first {
            out.push_str(&format!("    < {t}\n"));
        }
        if let Some(t) = right_first {
            out.push_str(&format!("    > {t}\n"));
        }
    }
}

fn render_dir_summary(out: &mut String, s: &DirSnapshot, opts: &ReportOptions) {
    out.push_str(&format!("左: {}  右: {}\n", s.left_label, s.right_label));
    out.push_str(&format!(
        "统计: 总数={} 相同={} 内容差异={} 快速差异={} 仅左={} 仅右={} 目录同={} 目录异={}\n",
        s.stats.total,
        s.stats.same,
        s.stats.content_diff,
        s.stats.quick_diff,
        s.stats.left_only,
        s.stats.right_only,
        s.stats.dir_same,
        s.stats.dir_diff
    ));
    if opts.dir_display == DirDisplayFilter::Matches {
        return;
    }
    out.push_str("差异清单:\n");
    for row in &s.rows {
        if !opts.dir_display.keeps(row) || row.status == ReportEntryStatus::Same || row.status == ReportEntryStatus::DirSame
        {
            continue;
        }
        out.push_str(&format!(
            "  [{status}] {path}\n",
            status = row.status.as_str(),
            path = row.display_path()
        ));
    }
}

// ---------------------------------------------------------------------------
// ② Patch：normal / context / unified（FR-PATCH-010）
// ---------------------------------------------------------------------------

pub struct PatchRenderer;

/// patch 脚本条目（与 CLI render_unified 同构；P1-10：上下文条目携带显式侧别
/// 下标——Equal 段左右起点在存在前置插入/删除后并不相等，不能共用左下标）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PatchEntry {
    /// 左侧上下文行：Equal 对齐行携带右下标 Some(r)（无换行标记双侧判定用）；
    /// ignore-unimportant 折叠行为 None
    SameLeft(u32, Option<u32>),
    /// 右侧上下文行（ignore-unimportant 折叠的插入行；计数与 SameLeft 同）
    SameRight(u32),
    Left(u32),
    Right(u32),
}

impl PatchEntry {
    /// 是否为上下文（计数同时消耗左右两侧行号）
    fn is_context(self) -> bool {
        matches!(self, PatchEntry::SameLeft(_, _) | PatchEntry::SameRight(_))
    }
}

/// unified patch 的无换行标记输出规则（GNU diff 语义）：
/// 行为该侧文件末行且该侧无换行符时输出；上下文行任一侧末行无换行即标记
/// （解析器对 context 前标记双侧置位）。
fn patch_no_newline_marker(
    out: &mut String,
    snap: &TextSnapshot,
    e: &PatchEntry,
    last_left: usize,
    last_right: usize,
) {
    let hit = match *e {
        PatchEntry::SameLeft(l, r) => {
            (l as usize == last_left && snap.left_no_trailing_newline)
                || r.is_some_and(|r| r as usize == last_right && snap.right_no_trailing_newline)
        }
        PatchEntry::SameRight(r) => r as usize == last_right && snap.right_no_trailing_newline,
        PatchEntry::Left(l) => l as usize == last_left && snap.left_no_trailing_newline,
        PatchEntry::Right(r) => r as usize == last_right && snap.right_no_trailing_newline,
    };
    if hit {
        out.push_str(NO_NEWLINE_MARKER);
    }
}

fn build_patch_script(snap: &TextSnapshot, opts: &ReportOptions) -> Vec<PatchEntry> {
    let fold = |sec: &Section| opts.ignore_unimportant && sec.important != Importance::Important;
    let mut script = Vec::new();
    for s in &snap.sections {
        match s.kind {
            SectionKind::Equal => {
                let l = s.left.expect("Equal 必有左区间");
                let r = s.right.expect("Equal 必有右区间");
                for i in 0..l.len() {
                    script.push(PatchEntry::SameLeft(l.start + i, Some(r.start + i)));
                }
            }
            SectionKind::Change | SectionKind::Delete => {
                let l = s.left.expect("Change/Delete 必有左区间");
                let r = s.right;
                if fold(s) {
                    // ignore-unimportant：整段折为上下文（左行优先，多余右行补右上下文；
                    // 语义：不重要变更不记入补丁，P2-29：保留真实区间不丢行）
                    for i in l.start..l.end {
                        script.push(PatchEntry::SameLeft(i, None));
                    }
                    if let Some(r) = r {
                        for i in r.start..r.end {
                            script.push(PatchEntry::SameRight(i));
                        }
                    }
                    continue;
                }
                for i in l.start..l.end {
                    script.push(PatchEntry::Left(i));
                }
                if s.kind == SectionKind::Change {
                    let r = r.expect("Change 必有右区间");
                    for i in r.start..r.end {
                        script.push(PatchEntry::Right(i));
                    }
                }
            }
            SectionKind::Insert => {
                let r = s.right.expect("Insert 必有右区间");
                if fold(s) {
                    for i in r.start..r.end {
                        script.push(PatchEntry::SameRight(i));
                    }
                    continue;
                }
                for i in r.start..r.end {
                    script.push(PatchEntry::Right(i));
                }
            }
        }
    }
    script
}

fn render_patch(snap: &TextSnapshot, opts: &ReportOptions) -> String {
    let script = build_patch_script(snap, opts);
    if script.iter().all(|e| e.is_context()) {
        return String::new(); // 无差异（或全为折叠的不重要差异）：patch 输出为空
    }
    match opts.patch_style {
        PatchStyle::Unified => render_unified_patch(snap, &script, opts),
        PatchStyle::Context => render_context_patch(snap, &script, opts),
        PatchStyle::Normal => render_normal_patch(snap, &script, opts),
    }
}

/// hunk 窗口选择（上下文合并；与 CLI render_unified 同构）
fn select_hunks(script: &[PatchEntry], context: u32) -> Vec<(usize, usize)> {
    let ctx = context as usize;
    let mut hunks: Vec<(usize, usize)> = Vec::new();
    for (idx, e) in script.iter().enumerate() {
        if !e.is_context() {
            let s = idx.saturating_sub(ctx);
            let e = (idx + ctx + 1).min(script.len());
            match hunks.last_mut() {
                Some((_, pe)) if s <= *pe => {
                    if e > *pe {
                        *pe = e;
                    }
                }
                _ => hunks.push((s, e)),
            }
        }
    }
    hunks
}

fn line_no_prefix(before: u32, count: u32, sign: char) -> String {
    // git 约定：count == 0 显示前导行数；count == 1 省略 ",1"
    let start = if count > 0 { before + 1 } else { before };
    if count == 1 {
        format!("{sign}{start}")
    } else {
        format!("{sign}{start},{count}")
    }
}

fn render_unified_patch(snap: &TextSnapshot, script: &[PatchEntry], opts: &ReportOptions) -> String {
    let mut out = String::new();
    out.push_str(&format!("--- {}\n", snap.left_label));
    out.push_str(&format!("+++ {}\n", snap.right_label));
    let last_left = snap.left_lines.len().saturating_sub(1);
    let last_right = snap.right_lines.len().saturating_sub(1);
    for (hs, he) in select_hunks(script, opts.context_lines) {
        let (mut lb, mut rb) = (0u32, 0u32);
        for e in &script[..hs] {
            match e {
                PatchEntry::SameLeft(..) | PatchEntry::SameRight(_) => {
                    lb += 1;
                    rb += 1;
                }
                PatchEntry::Left(_) => lb += 1,
                PatchEntry::Right(_) => rb += 1,
            }
        }
        let (mut lc, mut rc) = (0u32, 0u32);
        for e in &script[hs..he] {
            match e {
                PatchEntry::SameLeft(..) | PatchEntry::SameRight(_) => {
                    lc += 1;
                    rc += 1;
                }
                PatchEntry::Left(_) => lc += 1,
                PatchEntry::Right(_) => rc += 1,
            }
        }
        out.push_str(&format!(
            "@@ {} {} @@\n",
            line_no_prefix(lb, lc, '-'),
            line_no_prefix(rb, rc, '+')
        ));
        for e in &script[hs..he] {
            match e {
                PatchEntry::SameLeft(i, _) => out.push_str(&format!(" {}\n", snap.left_lines[*i as usize])),
                PatchEntry::SameRight(i) => out.push_str(&format!(" {}\n", snap.right_lines[*i as usize])),
                PatchEntry::Left(i) => out.push_str(&format!("-{}\n", snap.left_lines[*i as usize])),
                PatchEntry::Right(i) => out.push_str(&format!("+{}\n", snap.right_lines[*i as usize])),
            }
            patch_no_newline_marker(&mut out, snap, e, last_left, last_right);
        }
    }
    out
}

/// `\ No newline at end of file`（unified patch 约定行）
const NO_NEWLINE_MARKER: &str = "\\ No newline at end of file\n";

fn render_context_patch(snap: &TextSnapshot, script: &[PatchEntry], opts: &ReportOptions) -> String {
    let mut out = String::new();
    out.push_str(&format!("*** {}\n", snap.left_label));
    out.push_str(&format!("--- {}\n", snap.right_label));
    for (hs, he) in select_hunks(script, opts.context_lines) {
        let (mut lb, mut rb) = (0u32, 0u32);
        for e in &script[..hs] {
            match e {
                PatchEntry::SameLeft(..) | PatchEntry::SameRight(_) => {
                    lb += 1;
                    rb += 1;
                }
                PatchEntry::Left(_) => lb += 1,
                PatchEntry::Right(_) => rb += 1,
            }
        }
        let (mut lc, mut rc) = (0u32, 0u32);
        for e in &script[hs..he] {
            match e {
                PatchEntry::SameLeft(..) | PatchEntry::SameRight(_) => {
                    lc += 1;
                    rc += 1;
                }
                PatchEntry::Left(_) => lc += 1,
                PatchEntry::Right(_) => rc += 1,
            }
        }
        out.push_str("***************\n");
        // GNU context 格式：*** lstart,lend ****（count==0 显示前导行数，count==1 单数）
        let lhead = if lc == 0 {
            format!("{lb},0")
        } else if lc == 1 {
            format!("{}", lb + 1)
        } else {
            format!("{},{}", lb + 1, lb + lc)
        };
        out.push_str(&format!("*** {lhead} ****\n"));
        for e in &script[hs..he] {
            match e {
                // 上下文：左块显左行（SameRight 为折叠的右侧行，右块显之）
                PatchEntry::SameLeft(i, _) => out.push_str(&format!("  {}\n", snap.left_lines[*i as usize])),
                PatchEntry::SameRight(_) => {}
                PatchEntry::Left(i) => out.push_str(&format!("! {}\n", snap.left_lines[*i as usize])),
                PatchEntry::Right(_) => {}
            }
        }
        let rhead = if rc == 0 { format!("{rb},0") } else if rc == 1 { format!("{}", rb + 1) } else { format!("{},{}", rb + 1, rb + rc) };
        out.push_str(&format!("--- {rhead} ----\n"));
        for e in &script[hs..he] {
            match e {
                PatchEntry::SameLeft(..) => {}
                PatchEntry::SameRight(i) => out.push_str(&format!("  {}\n", snap.right_lines[*i as usize])),
                PatchEntry::Right(i) => out.push_str(&format!("! {}\n", snap.right_lines[*i as usize])),
                PatchEntry::Left(_) => {}
            }
        }
    }
    out
}

fn render_normal_patch(snap: &TextSnapshot, script: &[PatchEntry], opts: &ReportOptions) -> String {
    // normal diff：按连续 run 分组（改动 run = Left/Right 混合段）
    let _ = opts.context_lines; // normal 无上下文
    let mut out = String::new();
    let mut i = 0usize;
    let n = script.len();
    while i < n {
        if script[i].is_context() {
            i += 1;
            continue;
        }
        // 收集连续改动段
        let start = i;
        while i < n && !script[i].is_context() {
            i += 1;
        }
        let seg = &script[start..i];
        let (mut lb, mut lc) = (0u32, 0u32);
        let (mut rb, mut rc) = (0u32, 0u32);
        for e in &script[..start] {
            if e.is_context() {
                lb += 1;
                rb += 1;
            } else if matches!(e, PatchEntry::Left(_)) {
                lb += 1;
            } else {
                rb += 1;
            }
        }
        for e in seg {
            match e {
                PatchEntry::Left(_) => lc += 1,
                PatchEntry::Right(_) => rc += 1,
                _ => unreachable!("改动段不含上下文"),
            }
        }
        let lhead = if lc == 0 { format!("{lb}") } else if lc == 1 { format!("{}", lb + 1) } else { format!("{},{}", lb + 1, lc) };
        let rhead = if rc == 0 { format!("{rb}") } else if rc == 1 { format!("{}", rb + 1) } else { format!("{},{}", rb + 1, rc) };
        // a / c / d 命令字
        let cmd = match (lc, rc) {
            (0, _) => 'a',
            (_, 0) => 'd',
            _ => 'c',
        };
        out.push_str(&format!("{lhead}{cmd}{rhead}\n"));
        if lc > 0 {
            for e in seg.iter().filter(|e| matches!(e, PatchEntry::Left(_))) {
                if let PatchEntry::Left(li) = e {
                    out.push_str(&format!("< {}\n", snap.left_lines[*li as usize]));
                }
            }
            if rc > 0 {
                out.push_str("---\n");
            }
        }
        if rc > 0 {
            for e in seg.iter().filter(|e| matches!(e, PatchEntry::Right(_))) {
                if let PatchEntry::Right(ri) = e {
                    out.push_str(&format!("> {}\n", snap.right_lines[*ri as usize]));
                }
            }
        }
    }
    out
}

impl ReportRenderer for PatchRenderer {
    fn layout(&self) -> ReportLayout {
        ReportLayout::Patch
    }

    fn render(&self, snap: &ReportSnapshot, opts: &ReportOptions) -> Result<String, ReportError> {
        match snap {
            ReportSnapshot::Text(s) => Ok(render_patch(s, opts)),
            _ => Err(unsupported(ReportLayout::Patch, snap)),
        }
    }
}

// ---------------------------------------------------------------------------
// ③ Html：自包含（样式内联），色板语义 红=删/绿=增/蓝=改/灰=孤
// ---------------------------------------------------------------------------

pub struct HtmlRenderer;

/// HTML 转义（内容/属性安全）
pub fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

const HTML_CSS_COLOR_NONE: &str = "body{font-family:monospace}td{vertical-align:top;white-space:pre;}";
const HTML_CSS_COLOR_WORD: &str = "body{font-family:monospace}td{vertical-align:top;}td.t{white-space:pre-wrap;word-break:break-word;}";

fn html_stylesheet(color: ColorMode, wrap: WrapMode) -> String {
    let base = if wrap == WrapMode::Word { HTML_CSS_COLOR_WORD } else { HTML_CSS_COLOR_NONE };
    if color == ColorMode::Mono {
        format!("<style>{base}\n.del .t{{text-decoration:line-through}}\n.ins .t{{font-weight:bold}}\n.chg .t{{font-weight:bold}}\n.orphan{{color:#666}}\n</style>")
    } else {
        format!(
            "<style>{base}\n\
             h1{{font-family:sans-serif}}table{{border-collapse:collapse;width:100%}}\n\
             td,th{{border:1px solid #ccc;padding:1px 6px;font-family:monospace}}\n\
             th{{background:#eee}}\n\
             tr.del{{background:#ffd6d6}}tr.ins{{background:#d6ffd6}}tr.chg{{background:#d6e4ff}}\n\
             tr.same{{}}tr.orphan{{background:#eee}}tr.unimp{{background:#fff3c2}}\n\
             .meta{{color:#555}}\n\
             </style>"
        )
    }
}

fn html_head(title: &str, color: ColorMode, wrap: WrapMode) -> String {
    format!(
        "<!DOCTYPE html>\n<html>\n<head>\n<meta charset=\"utf-8\">\n<title>{}</title>\n{}</head>\n",
        html_escape(title),
        html_stylesheet(color, wrap)
    )
}

impl ReportRenderer for HtmlRenderer {
    fn layout(&self) -> ReportLayout {
        ReportLayout::Html
    }

    fn render(&self, snap: &ReportSnapshot, opts: &ReportOptions) -> Result<String, ReportError> {
        let title = effective_title(snap, opts);
        let mut out = html_head(&title, opts.color, opts.wrap);
        out.push_str("<body>\n");
        out.push_str(&format!("<h1>{}</h1>\n", html_escape(&title)));
        match snap {
            ReportSnapshot::Text(s) => html_text(&mut out, s, opts),
            ReportSnapshot::Dir(s) => html_dir(&mut out, s, opts),
            ReportSnapshot::Table(s) => html_table(&mut out, s, opts),
            ReportSnapshot::Hex(s) => html_hex(&mut out, s, opts),
            ReportSnapshot::Merge(s) => html_merge(&mut out, s, opts),
        }
        out.push_str("</body>\n</html>\n");
        Ok(out)
    }
}

fn row_class(kind: SectionKind, important: Importance, ignore_unimportant: bool) -> &'static str {
    if kind != SectionKind::Equal && important != Importance::Important && ignore_unimportant {
        return "unimp";
    }
    match kind {
        SectionKind::Equal => "same",
        SectionKind::Insert => "ins",
        SectionKind::Delete => "del",
        SectionKind::Change => "chg",
    }
}

/// HTML 双栏装配行（行号/文本 × 两侧 + 行类别）
type HtmlDiffRow = (Option<u32>, String, Option<u32>, String, &'static str);

fn html_text(out: &mut String, s: &TextSnapshot, opts: &ReportOptions) {
    out.push_str(&format!(
        "<p class=\"meta\">左: {}　右: {}</p>\n",
        html_escape(&s.left_label),
        html_escape(&s.right_label)
    ));
    let vis = visible_text_sections(s, opts);
    out.push_str("<table class=\"diff\">\n");
    if opts.line_numbers {
        out.push_str("<tr><th>#</th><th>左</th><th>#</th><th>右</th></tr>\n");
    } else {
        out.push_str("<tr><th>左</th><th>右</th></tr>\n");
    }
    // 双栏装配：逐 section 展开为 (leftNo, leftText, rightNo, rightText, class) 行
    let mut rows: Vec<HtmlDiffRow> = Vec::new();
    for sec in &vis {
        let class = row_class(sec.kind, sec.important, opts.ignore_unimportant);
        let lefts = lines_of(s, crate::options::DiffSide::Left, sec);
        let rights = lines_of(s, crate::options::DiffSide::Right, sec);
        match sec.kind {
            SectionKind::Equal => {
                for (i, t) in lefts.iter().enumerate() {
                    let no = sec.left.map(|r| r.start + i as u32 + 1);
                    rows.push((no, t.clone(), no, t.clone(), class));
                }
            }
            SectionKind::Change => {
                let max = lefts.len().max(rights.len());
                for i in 0..max {
                    let ln = sec.left.filter(|_| i < lefts.len()).map(|r| r.start + i as u32 + 1);
                    let rn = sec.right.filter(|_| i < rights.len()).map(|r| r.start + i as u32 + 1);
                    rows.push((
                        ln,
                        lefts.get(i).cloned().unwrap_or_default(),
                        rn,
                        rights.get(i).cloned().unwrap_or_default(),
                        class,
                    ));
                }
            }
            SectionKind::Delete => {
                for (i, t) in lefts.iter().enumerate() {
                    let no = sec.left.map(|r| r.start + i as u32 + 1);
                    rows.push((no, t.clone(), None, String::new(), class));
                }
            }
            SectionKind::Insert => {
                for (i, t) in rights.iter().enumerate() {
                    let no = sec.right.map(|r| r.start + i as u32 + 1);
                    rows.push((None, String::new(), no, t.clone(), class));
                }
            }
        }
    }
    for (ln, lt, rn, rt, class) in rows {
        if opts.line_numbers {
            out.push_str(&format!(
                "<tr class=\"{class}\"><td>{}</td><td class=\"t\">{}</td><td>{}</td><td class=\"t\">{}</td></tr>\n",
                ln.map(|v| v.to_string()).unwrap_or_default(),
                html_escape(&lt),
                rn.map(|v| v.to_string()).unwrap_or_default(),
                html_escape(&rt)
            ));
        } else {
            out.push_str(&format!(
                "<tr class=\"{class}\"><td class=\"t\">{}</td><td class=\"t\">{}</td></tr>\n",
                html_escape(&lt),
                html_escape(&rt)
            ));
        }
    }
    out.push_str("</table>\n");
}

fn html_dir(out: &mut String, s: &DirSnapshot, opts: &ReportOptions) {
    out.push_str(&format!(
        "<p class=\"meta\">左: {}　右: {}</p>\n",
        html_escape(&s.left_label),
        html_escape(&s.right_label)
    ));
    out.push_str("<table class=\"dir\">\n<tr><th>路径</th><th>状态</th>");
    if opts.columns.size {
        out.push_str("<th>左大小</th><th>右大小</th>");
    }
    if opts.columns.timestamp {
        out.push_str("<th>左时间</th><th>右时间</th>");
    }
    if opts.columns.attributes {
        out.push_str("<th>左属性</th><th>右属性</th>");
    }
    out.push_str("</tr>\n");
    for row in &s.rows {
        if !opts.dir_display.keeps(row) {
            continue;
        }
        let orphan = status_is_orphan(row.status);
        let class = if orphan {
            "orphan"
        } else if status_is_diff(row.status) {
            "chg"
        } else {
            "same"
        };
        let name_cell = if opts.include_file_links && row.kind == ReportEntryKind::File && !orphan {
            format!("<a href=\"{}.html\">{}</a>", html_escape(&row.rel_path), html_escape(&row.name))
        } else {
            html_escape(&row.display_path())
        };
        out.push_str(&format!("<tr class=\"{class}\"><td>{name_cell}</td><td>{}</td>", html_escape(row.status.as_str())));
        if opts.columns.size {
            out.push_str(&format!(
                "<td>{}</td><td>{}</td>",
                row.left.as_ref().map(|i| i.size.to_string()).unwrap_or_default(),
                row.right.as_ref().map(|i| i.size.to_string()).unwrap_or_default()
            ));
        }
        if opts.columns.timestamp {
            out.push_str(&format!(
                "<td>{}</td><td>{}</td>",
                row.left.as_ref().map(|i| i.mtime_ms.to_string()).unwrap_or_default(),
                row.right.as_ref().map(|i| i.mtime_ms.to_string()).unwrap_or_default()
            ));
        }
        if opts.columns.attributes {
            out.push_str(&format!(
                "<td>{}</td><td>{}</td>",
                row.left.as_ref().and_then(|i| i.attribute.clone()).unwrap_or_default(),
                row.right.as_ref().and_then(|i| i.attribute.clone()).unwrap_or_default()
            ));
        }
        out.push_str("</tr>\n");
    }
    out.push_str("</table>\n");
}

fn html_table(out: &mut String, s: &TableSnapshot, opts: &ReportOptions) {
    out.push_str(&format!(
        "<p class=\"meta\">左: {}　右: {}</p>\n",
        html_escape(&s.left_label),
        html_escape(&s.right_label)
    ));
    out.push_str("<table class=\"table-diff\">\n<tr><th>#</th><th>#</th>");
    for col in &s.columns {
        out.push_str(&format!("<th>{}</th>", html_escape(&col.name)));
    }
    out.push_str("</tr>\n");
    for row in &s.rows {
        let class = match row.kind {
            crate::table::TableRowKind::Same => "same",
            crate::table::TableRowKind::Changed => {
                if !row.important && opts.ignore_unimportant {
                    "unimp"
                } else {
                    "chg"
                }
            }
            crate::table::TableRowKind::Added => "ins",
            crate::table::TableRowKind::Removed => "del",
        };
        out.push_str(&format!(
            "<tr class=\"{class}\"><td>{}</td><td>{}</td>",
            row.left.map(|v| v.to_string()).unwrap_or_default(),
            row.right.map(|v| v.to_string()).unwrap_or_default()
        ));
        for cell in &row.cells {
            let (l, r) = (cell.left.clone().unwrap_or_default(), cell.right.clone().unwrap_or_default());
            let _ = opts;
            // 单元格显示：有差异时 "L | R"
            let shown = if l == r { l } else { format!("{l} | {r}") };
            out.push_str(&format!("<td>{}</td>", html_escape(&shown)));
        }
        out.push_str("</tr>\n");
    }
    out.push_str("</table>\n");
}

fn byte_status_class(st: HexByteStatus) -> &'static str {
    match st {
        HexByteStatus::Same => "hs",
        HexByteStatus::Changed => "hc",
        HexByteStatus::Inserted => "hi",
        HexByteStatus::Deleted => "hd",
    }
}

fn html_hex_side(out: &mut String, label: &str, data: &[u8], status: &[HexByteStatus], opts: &ReportOptions) {
    out.push_str(&format!("<p class=\"meta\">{}</p>\n", html_escape(label)));
    out.push_str("<table class=\"hex\">\n");
    let lines = data.len().div_ceil(HEX_LINE_BYTES);
    for li in 0..lines {
        let start = li * HEX_LINE_BYTES;
        let end = (start + HEX_LINE_BYTES).min(data.len());
        let addr = format!("{:08x}", start);
        let mut hex = String::new();
        let mut ascii = String::new();
        let mut spans: Vec<(HexByteStatus, String)> = Vec::new();
        for (i, b) in data[start..end].iter().enumerate() {
            hex.push_str(&format!("{b:02x} "));
            let c = if b.is_ascii_graphic() || *b == b' ' { *b as char } else { '.' };
            ascii.push(c);
            let st = status.get(start + i).copied().unwrap_or(HexByteStatus::Same);
            match spans.last_mut() {
                Some((s, buf)) if *s == st => buf.push_str(&format!("{b:02x} ")),
                _ => spans.push((st, format!("{b:02x} "))),
            }
        }
        let _ = hex;
        let hex_cell = spans
            .iter()
            .map(|(st, buf)| format!("<span class=\"{}\">{}</span>", byte_status_class(*st), buf))
            .collect::<Vec<_>>()
            .join("");
        let addr_cell = if opts.line_numbers {
            format!("<td>{addr}</td>")
        } else {
            String::new()
        };
        out.push_str(&format!(
            "<tr>{addr_cell}<td class=\"t\">{hex_cell}</td><td class=\"t\">{}</td></tr>\n",
            html_escape(&ascii)
        ));
    }
    out.push_str("</table>\n");
}

fn html_hex(out: &mut String, s: &HexSnapshot, opts: &ReportOptions) {
    html_hex_side(out, &format!("左: {}", s.left_label), &s.left, &s.status_left, opts);
    html_hex_side(out, &format!("右: {}", s.right_label), &s.right, &s.status_right, opts);
}

fn html_merge(out: &mut String, s: &MergeSnapshot, _opts: &ReportOptions) {
    out.push_str(&format!(
        "<p class=\"meta\">基: {}　左: {}　右: {}</p>\n",
        html_escape(&s.base_label),
        html_escape(&s.left_label),
        html_escape(&s.right_label)
    ));
    out.push_str("<table class=\"merge\">\n<tr><th>#</th><th>来源</th><th>base</th><th>left</th><th>right</th><th>状态</th></tr>\n");
    for c in &s.model.chunks {
        let class = match c.origin {
            ChunkOrigin::Stable => "same",
            ChunkOrigin::Conflict => if s.unresolved.contains(&c.id) { "del" } else { "chg" },
            _ => "ins",
        };
        out.push_str(&format!(
            "<tr class=\"{class}\"><td>{}</td><td>{}</td><td>{}-{}</td><td>{}-{}</td><td>{}-{}</td><td>{}</td></tr>\n",
            c.id,
            html_escape(merge_origin_str(c.origin)),
            c.base.start,
            c.base.end,
            c.left.start,
            c.left.end,
            c.right.start,
            c.right.end,
            if s.unresolved.contains(&c.id) { "未决" } else { "已决" }
        ));
    }
    out.push_str("</table>\n");
}

// ---------------------------------------------------------------------------
// ④ Xml：结构化（全部转义）
// ---------------------------------------------------------------------------

pub struct XmlRenderer;

fn xml_escape(s: &str) -> String {
    html_escape(s)
}

fn xml_attr_row(row: &DirReportRow, indent: &str, out: &mut String) {
    out.push_str(&format!("{}<row path=\"{}\" name=\"{}\" kind=\"{}\" status=\"{}\"",
        indent,
        xml_escape(&row.rel_path),
        xml_escape(&row.name),
        row.kind.as_str(),
        row.status.as_str()));
    if let Some(l) = &row.left {
        out.push_str(&format!(" leftSize=\"{}\" leftMtimeMs=\"{}\"", l.size, l.mtime_ms));
    }
    if let Some(r) = &row.right {
        out.push_str(&format!(" rightSize=\"{}\" rightMtimeMs=\"{}\"", r.size, r.mtime_ms));
    }
    out.push_str("/>\n");
}

impl ReportRenderer for XmlRenderer {
    fn layout(&self) -> ReportLayout {
        ReportLayout::Xml
    }

    fn render(&self, snap: &ReportSnapshot, opts: &ReportOptions) -> Result<String, ReportError> {
        let title = effective_title(snap, opts);
        let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
        match snap {
            ReportSnapshot::Text(s) => {
                out.push_str(&format!("<report kind=\"text\" title=\"{}\" left=\"{}\" right=\"{}\">\n",
                    xml_escape(&title), xml_escape(&s.left_label), xml_escape(&s.right_label)));
                out.push_str("<summary");
                for (k, sec) in [("equal", SectionKind::Equal), ("insert", SectionKind::Insert),
                                 ("delete", SectionKind::Delete), ("change", SectionKind::Change)] {
                    let count = s.sections.iter().filter(|x| x.kind == sec).count();
                    out.push_str(&format!(" {k}Sections=\"{count}\""));
                }
                out.push_str(&format!(" unimportantSections=\"{}\"/>\n", s.unimportant_sections));
                out.push_str("<sections>\n");
                for sec in visible_text_sections(s, opts) {
                    out.push_str(&format!(
                        "  <section kind=\"{}\" important=\"{}\"",
                        section_kind_str(sec.kind),
                        sec.important == Importance::Important
                    ));
                    if let Some(l) = sec.left {
                        out.push_str(&format!(" leftStart=\"{}\" leftEnd=\"{}\"", l.start, l.end));
                    }
                    if let Some(r) = sec.right {
                        out.push_str(&format!(" rightStart=\"{}\" rightEnd=\"{}\"", r.start, r.end));
                    }
                    out.push_str(">\n");
                    for (i, t) in lines_of(s, crate::options::DiffSide::Left, sec).iter().enumerate() {
                        let no = sec.left.map(|r| r.start + i as u32 + 1).unwrap_or(0);
                        out.push_str(&format!("    <leftLine no=\"{no}\">{}</leftLine>\n", xml_escape(t)));
                    }
                    for (i, t) in lines_of(s, crate::options::DiffSide::Right, sec).iter().enumerate() {
                        let no = sec.right.map(|r| r.start + i as u32 + 1).unwrap_or(0);
                        out.push_str(&format!("    <rightLine no=\"{no}\">{}</rightLine>\n", xml_escape(t)));
                    }
                    out.push_str("  </section>\n");
                }
                out.push_str("</sections>\n</report>\n");
            }
            ReportSnapshot::Dir(s) => {
                out.push_str(&format!("<report kind=\"folder\" title=\"{}\" left=\"{}\" right=\"{}\">\n",
                    xml_escape(&title), xml_escape(&s.left_label), xml_escape(&s.right_label)));
                out.push_str(&format!(
                    "<summary total=\"{}\" same=\"{}\" contentDiff=\"{}\" quickDiff=\"{}\" leftOnly=\"{}\" rightOnly=\"{}\" dirSame=\"{}\" dirDiff=\"{}\"/>\n",
                    s.stats.total, s.stats.same, s.stats.content_diff, s.stats.quick_diff,
                    s.stats.left_only, s.stats.right_only, s.stats.dir_same, s.stats.dir_diff
                ));
                out.push_str("<rows>\n");
                for row in &s.rows {
                    if opts.dir_display.keeps(row) {
                        xml_attr_row(row, "  ", &mut out);
                    }
                }
                out.push_str("</rows>\n</report>\n");
            }
            ReportSnapshot::Table(s) => {
                out.push_str(&format!("<report kind=\"table\" title=\"{}\" left=\"{}\" right=\"{}\">\n",
                    xml_escape(&title), xml_escape(&s.left_label), xml_escape(&s.right_label)));
                out.push_str(&format!(
                    "<summary sameRows=\"{}\" changedRows=\"{}\" addedRows=\"{}\" removedRows=\"{}\" unimportantRows=\"{}\"/>\n",
                    s.summary.same_rows, s.summary.changed_rows, s.summary.added_rows,
                    s.summary.removed_rows, s.summary.unimportant_rows
                ));
                out.push_str("<columns>\n");
                for (i, col) in s.columns.iter().enumerate() {
                    out.push_str(&format!(
                        "  <column index=\"{i}\" name=\"{}\" leftIndex=\"{}\" rightIndex=\"{}\"/>\n",
                        xml_escape(&col.name),
                        col.left.map(|v| v.to_string()).unwrap_or_default(),
                        col.right.map(|v| v.to_string()).unwrap_or_default()
                    ));
                }
                out.push_str("</columns>\n<rows>\n");
                for row in &s.rows {
                    out.push_str(&format!("  <row kind=\"{}\"", row.kind.as_str()));
                    if let Some(l) = row.left {
                        out.push_str(&format!(" leftRow=\"{l}\""));
                    }
                    if let Some(r) = row.right {
                        out.push_str(&format!(" rightRow=\"{r}\""));
                    }
                    out.push_str(">\n");
                    for (i, cell) in row.cells.iter().enumerate() {
                        out.push_str(&format!(
                            "    <cell col=\"{i}\" status=\"{}\" left=\"{}\" right=\"{}\"/>\n",
                            cell.status.as_str(),
                            xml_escape(&cell.left.clone().unwrap_or_default()),
                            xml_escape(&cell.right.clone().unwrap_or_default())
                        ));
                    }
                    out.push_str("  </row>\n");
                }
                out.push_str("</rows>\n</report>\n");
            }
            ReportSnapshot::Hex(s) => {
                out.push_str(&format!("<report kind=\"hex\" title=\"{}\" left=\"{}\" right=\"{}\">\n",
                    xml_escape(&title), xml_escape(&s.left_label), xml_escape(&s.right_label)));
                out.push_str(&format!(
                    "<summary sameBytes=\"{}\" changedBytes=\"{}\" insertedBytes=\"{}\" deletedBytes=\"{}\" blocks=\"{}\"/>\n",
                    s.model.summary.same_bytes, s.model.summary.changed_bytes,
                    s.model.summary.inserted_bytes, s.model.summary.deleted_bytes,
                    s.model.blocks.len()
                ));
                out.push_str("<blocks>\n");
                for b in &s.model.blocks {
                    out.push_str(&format!("  <block kind=\"{}\"", hex_block_kind_str(b.kind)));
                    if let Some(l) = &b.left {
                        out.push_str(&format!(" leftOffset=\"{}\" leftLen=\"{}\"", l.offset, l.len));
                    }
                    if let Some(r) = &b.right {
                        out.push_str(&format!(" rightOffset=\"{}\" rightLen=\"{}\"", r.offset, r.len));
                    }
                    out.push_str("/>\n");
                }
                out.push_str("</blocks>\n</report>\n");
            }
            ReportSnapshot::Merge(s) => {
                out.push_str(&format!("<report kind=\"merge\" title=\"{}\" base=\"{}\" left=\"{}\" right=\"{}\">\n",
                    xml_escape(&title), xml_escape(&s.base_label), xml_escape(&s.left_label), xml_escape(&s.right_label)));
                out.push_str(&format!(
                    "<summary chunks=\"{}\" conflicts=\"{}\" unresolved=\"{}\"/>\n",
                    s.model.chunks.len(),
                    s.model.conflict_count(),
                    s.unresolved.len()
                ));
                out.push_str("<chunks>\n");
                for c in &s.model.chunks {
                    out.push_str(&format!(
                        "  <chunk id=\"{}\" origin=\"{}\" baseStart=\"{}\" baseEnd=\"{}\" leftStart=\"{}\" leftEnd=\"{}\" rightStart=\"{}\" rightEnd=\"{}\" unresolved=\"{}\"/>\n",
                        c.id,
                        merge_origin_str(c.origin),
                        c.base.start, c.base.end, c.left.start, c.left.end, c.right.start, c.right.end,
                        s.unresolved.contains(&c.id)
                    ));
                }
                out.push_str("</chunks>\n</report>\n");
            }
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// ⑤ Csv / TSV：RFC4180 转义
// ---------------------------------------------------------------------------

pub struct CsvRenderer;

fn csv_escape(v: &str, delim: char) -> String {
    if v.contains(delim) || v.contains('"') || v.contains('\n') || v.contains('\r') {
        format!("\"{}\"", v.replace('"', "\"\""))
    } else {
        v.to_string()
    }
}

fn csv_row(cells: &[String], delim: char) -> String {
    cells
        .iter()
        .map(|c| csv_escape(c, delim))
        .collect::<Vec<_>>()
        .join(&delim.to_string())
        + "\n"
}

impl ReportRenderer for CsvRenderer {
    fn layout(&self) -> ReportLayout {
        ReportLayout::Csv
    }

    fn render(&self, snap: &ReportSnapshot, opts: &ReportOptions) -> Result<String, ReportError> {
        let d = opts.delimiter;
        let mut out = String::new();
        match snap {
            ReportSnapshot::Text(s) => {
                out.push_str(&csv_row(&[
                    "kind".into(), "leftStart".into(), "leftCount".into(),
                    "rightStart".into(), "rightCount".into(), "leftText".into(), "rightText".into(),
                ], d));
                for sec in visible_text_sections(s, opts) {
                    let ltext = lines_of(s, crate::options::DiffSide::Left, sec).join("\n");
                    let rtext = lines_of(s, crate::options::DiffSide::Right, sec).join("\n");
                    out.push_str(&csv_row(&[
                        section_kind_str(sec.kind).to_string(),
                        sec.left.map(|r| r.start.to_string()).unwrap_or_default(),
                        sec.left.map(|r| r.len().to_string()).unwrap_or_default(),
                        sec.right.map(|r| r.start.to_string()).unwrap_or_default(),
                        sec.right.map(|r| r.len().to_string()).unwrap_or_default(),
                        ltext,
                        rtext,
                    ], d));
                }
            }
            ReportSnapshot::Dir(s) => {
                let mut header = vec!["path".to_string(), "name".to_string(), "kind".to_string(), "status".to_string()];
                if opts.columns.size {
                    header.push("leftSize".into());
                    header.push("rightSize".into());
                }
                if opts.columns.timestamp {
                    header.push("leftMtimeMs".into());
                    header.push("rightMtimeMs".into());
                }
                out.push_str(&csv_row(&header, d));
                for row in &s.rows {
                    if !opts.dir_display.keeps(row) {
                        continue;
                    }
                    let mut cells = vec![
                        row.rel_path.clone(),
                        row.name.clone(),
                        row.kind.as_str().to_string(),
                        row.status.as_str().to_string(),
                    ];
                    if opts.columns.size {
                        cells.push(row.left.as_ref().map(|i| i.size.to_string()).unwrap_or_default());
                        cells.push(row.right.as_ref().map(|i| i.size.to_string()).unwrap_or_default());
                    }
                    if opts.columns.timestamp {
                        cells.push(row.left.as_ref().map(|i| i.mtime_ms.to_string()).unwrap_or_default());
                        cells.push(row.right.as_ref().map(|i| i.mtime_ms.to_string()).unwrap_or_default());
                    }
                    out.push_str(&csv_row(&cells, d));
                }
            }
            ReportSnapshot::Table(s) => {
                let mut header = vec!["rowKind".to_string(), "leftRow".to_string(), "rightRow".to_string()];
                for col in &s.columns {
                    header.push(format!("{}|left", col.name));
                    header.push(format!("{}|right", col.name));
                }
                out.push_str(&csv_row(&header, d));
                for row in &s.rows {
                    let mut cells = vec![
                        row.kind.as_str().to_string(),
                        row.left.map(|v| v.to_string()).unwrap_or_default(),
                        row.right.map(|v| v.to_string()).unwrap_or_default(),
                    ];
                    for cell in &row.cells {
                        cells.push(cell.left.clone().unwrap_or_default());
                        cells.push(cell.right.clone().unwrap_or_default());
                    }
                    out.push_str(&csv_row(&cells, d));
                }
            }
            ReportSnapshot::Hex(s) => {
                out.push_str(&csv_row(&[
                    "kind".into(), "leftOffset".into(), "leftLen".into(),
                    "rightOffset".into(), "rightLen".into(),
                ], d));
                for b in &s.model.blocks {
                    out.push_str(&csv_row(&[
                        hex_block_kind_str(b.kind).to_string(),
                        b.left.as_ref().map(|l| l.offset.to_string()).unwrap_or_default(),
                        b.left.as_ref().map(|l| l.len.to_string()).unwrap_or_default(),
                        b.right.as_ref().map(|r| r.offset.to_string()).unwrap_or_default(),
                        b.right.as_ref().map(|r| r.len.to_string()).unwrap_or_default(),
                    ], d));
                }
            }
            ReportSnapshot::Merge(s) => {
                out.push_str(&csv_row(&[
                    "id".into(), "origin".into(), "baseStart".into(), "baseEnd".into(),
                    "leftStart".into(), "leftEnd".into(), "rightStart".into(), "rightEnd".into(), "unresolved".into(),
                ], d));
                for c in &s.model.chunks {
                    out.push_str(&csv_row(&[
                        c.id.to_string(),
                        merge_origin_str(c.origin).to_string(),
                        c.base.start.to_string(),
                        c.base.end.to_string(),
                        c.left.start.to_string(),
                        c.left.end.to_string(),
                        c.right.start.to_string(),
                        c.right.end.to_string(),
                        if s.unresolved.contains(&c.id) { "true".into() } else { "false".into() },
                    ], d));
                }
            }
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// ⑥ Tree：文件夹缩进树 + 状态标记
// ---------------------------------------------------------------------------

pub struct TreeRenderer;

/// 状态标记（树/清单共用）：= 同 / ! 差异 / < 仅左 / > 仅右 / ? 未判定
pub fn status_mark(s: ReportEntryStatus) -> &'static str {
    match s {
        ReportEntryStatus::Same | ReportEntryStatus::DirSame => "=",
        ReportEntryStatus::QuickDiff | ReportEntryStatus::ContentDiff | ReportEntryStatus::DirDiff => "!",
        ReportEntryStatus::LeftOnly => "<",
        ReportEntryStatus::RightOnly => ">",
        ReportEntryStatus::Unknown => "?",
    }
}

impl ReportRenderer for TreeRenderer {
    fn layout(&self) -> ReportLayout {
        ReportLayout::Tree
    }

    fn render(&self, snap: &ReportSnapshot, opts: &ReportOptions) -> Result<String, ReportError> {
        let s = match snap {
            ReportSnapshot::Dir(s) => s,
            _ => return Err(unsupported(ReportLayout::Tree, snap)),
        };
        let mut out = String::new();
        out.push_str(&format!("{}\n", effective_title(snap, opts)));
        for row in &s.rows {
            if !opts.dir_display.keeps(row) {
                continue;
            }
            let depth = row.rel_path.split('/').filter(|x| !x.is_empty()).count().saturating_sub(1);
            let indent = "  ".repeat(depth);
            let slash = if row.kind == ReportEntryKind::Dir { "/" } else { "" };
            out.push_str(&format!("{indent}[{}] {}{}\n", status_mark(row.status), row.name, slash));
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// ⑦ List：文件清单
// ---------------------------------------------------------------------------

pub struct ListRenderer;

impl ReportRenderer for ListRenderer {
    fn layout(&self) -> ReportLayout {
        ReportLayout::List
    }

    fn render(&self, snap: &ReportSnapshot, opts: &ReportOptions) -> Result<String, ReportError> {
        let mut out = String::new();
        match snap {
            ReportSnapshot::Dir(s) => {
                out.push_str(&format!("{}\n", effective_title(snap, opts)));
                for row in &s.rows {
                    if !opts.dir_display.keeps(row) {
                        continue;
                    }
                    let lsize = row.left.as_ref().map(|i| i.size.to_string()).unwrap_or_else(|| "-".into());
                    let rsize = row.right.as_ref().map(|i| i.size.to_string()).unwrap_or_else(|| "-".into());
                    out.push_str(&format!(
                        "{}\t{}\t{}\t{}\n",
                        row.status.as_str(),
                        row.display_path(),
                        lsize,
                        rsize
                    ));
                }
            }
            ReportSnapshot::Text(s) => {
                out.push_str(&format!("{}\n", effective_title(snap, opts)));
                for sec in visible_text_sections(s, opts) {
                    if sec.kind == SectionKind::Equal {
                        continue;
                    }
                    out.push_str(&format!(
                        "{}\tL{}-{}\tR{}-{}\n",
                        section_kind_str(sec.kind),
                        sec.left.map(|r| r.start + 1).unwrap_or(0),
                        sec.left.map(|r| r.end).unwrap_or(0),
                        sec.right.map(|r| r.start + 1).unwrap_or(0),
                        sec.right.map(|r| r.end).unwrap_or(0),
                    ));
                }
            }
            ReportSnapshot::Table(s) => {
                out.push_str(&format!("{}\n", effective_title(snap, opts)));
                for row in &s.rows {
                    if row.kind == crate::table::TableRowKind::Same {
                        continue;
                    }
                    out.push_str(&format!(
                        "{}\tL{}\tR{}\n",
                        row.kind.as_str(),
                        row.left.map(|v| v.to_string()).unwrap_or_else(|| "-".into()),
                        row.right.map(|v| v.to_string()).unwrap_or_else(|| "-".into()),
                    ));
                }
            }
            ReportSnapshot::Hex(s) => {
                out.push_str(&format!("{}\n", effective_title(snap, opts)));
                for b in &s.model.blocks {
                    if b.kind == HexBlockKind::Same {
                        continue;
                    }
                    out.push_str(&format!(
                        "{}\tL{}\tR{}\n",
                        hex_block_kind_str(b.kind),
                        b.left.as_ref().map(|l| format!("0x{:x}+{}", l.offset, l.len)).unwrap_or_else(|| "-".into()),
                        b.right.as_ref().map(|r| format!("0x{:x}+{}", r.offset, r.len)).unwrap_or_else(|| "-".into()),
                    ));
                }
            }
            ReportSnapshot::Merge(s) => {
                out.push_str(&format!("{}\n", effective_title(snap, opts)));
                for c in &s.model.chunks {
                    if c.origin == ChunkOrigin::Stable {
                        continue;
                    }
                    out.push_str(&format!(
                        "{}\t#{}\tL{}-{}\tR{}-{}\n",
                        merge_origin_str(c.origin),
                        c.id,
                        c.left.start,
                        c.left.end,
                        c.right.start,
                        c.right.end
                    ));
                }
            }
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// ⑧ Stats：只统计无内容
// ---------------------------------------------------------------------------

pub struct StatsRenderer;

impl ReportRenderer for StatsRenderer {
    fn layout(&self) -> ReportLayout {
        ReportLayout::Stats
    }

    fn render(&self, snap: &ReportSnapshot, opts: &ReportOptions) -> Result<String, ReportError> {
        let _ = opts;
        let mut out = String::new();
        match snap {
            ReportSnapshot::Text(s) => {
                let mut counts = [0u32; 4];
                for sec in &s.sections {
                    let idx = match sec.kind {
                        SectionKind::Equal => 0,
                        SectionKind::Insert => 1,
                        SectionKind::Delete => 2,
                        SectionKind::Change => 3,
                    };
                    counts[idx] += 1;
                }
                let inserted: u32 = s.sections.iter().map(|s| match s.kind {
                    SectionKind::Insert => s.right.map_or(0, |r| r.len()),
                    SectionKind::Change => s.right.map_or(0, |r| r.len()),
                    _ => 0,
                }).sum();
                let deleted: u32 = s.sections.iter().map(|s| match s.kind {
                    SectionKind::Delete => s.left.map_or(0, |r| r.len()),
                    SectionKind::Change => s.left.map_or(0, |r| r.len()),
                    _ => 0,
                }).sum();
                let equal: u32 = s.sections.iter().map(|s| match s.kind {
                    SectionKind::Equal => s.left.map_or(0, |r| r.len()),
                    _ => 0,
                }).sum();
                out.push_str(&format!("sections={}\n", s.sections.len()));
                out.push_str(&format!("equalSections={}\n", counts[0]));
                out.push_str(&format!("insertSections={}\n", counts[1]));
                out.push_str(&format!("deleteSections={}\n", counts[2]));
                out.push_str(&format!("changeSections={}\n", counts[3]));
                out.push_str(&format!("equalLines={equal}\n"));
                out.push_str(&format!("insertedLines={inserted}\n"));
                out.push_str(&format!("deletedLines={deleted}\n"));
                out.push_str(&format!("unimportantSections={}\n", s.unimportant_sections));
                out.push_str(&format!("hasDifferences={}\n", snap.has_differences()));
            }
            ReportSnapshot::Dir(s) => {
                out.push_str(&format!("total={}\n", s.stats.total));
                out.push_str(&format!("same={}\n", s.stats.same));
                out.push_str(&format!("contentDiff={}\n", s.stats.content_diff));
                out.push_str(&format!("quickDiff={}\n", s.stats.quick_diff));
                out.push_str(&format!("leftOnly={}\n", s.stats.left_only));
                out.push_str(&format!("rightOnly={}\n", s.stats.right_only));
                out.push_str(&format!("dirSame={}\n", s.stats.dir_same));
                out.push_str(&format!("dirDiff={}\n", s.stats.dir_diff));
                out.push_str(&format!("hasDifferences={}\n", snap.has_differences()));
            }
            ReportSnapshot::Table(s) => {
                out.push_str(&format!("sameRows={}\n", s.summary.same_rows));
                out.push_str(&format!("changedRows={}\n", s.summary.changed_rows));
                out.push_str(&format!("addedRows={}\n", s.summary.added_rows));
                out.push_str(&format!("removedRows={}\n", s.summary.removed_rows));
                out.push_str(&format!("unimportantRows={}\n", s.summary.unimportant_rows));
                out.push_str(&format!("hasDifferences={}\n", snap.has_differences()));
            }
            ReportSnapshot::Hex(s) => {
                out.push_str(&format!("sameBytes={}\n", s.model.summary.same_bytes));
                out.push_str(&format!("changedBytes={}\n", s.model.summary.changed_bytes));
                out.push_str(&format!("insertedBytes={}\n", s.model.summary.inserted_bytes));
                out.push_str(&format!("deletedBytes={}\n", s.model.summary.deleted_bytes));
                out.push_str(&format!("blocks={}\n", s.model.blocks.len()));
                out.push_str(&format!("hasDifferences={}\n", snap.has_differences()));
            }
            ReportSnapshot::Merge(s) => {
                out.push_str(&format!("chunks={}\n", s.model.chunks.len()));
                out.push_str(&format!("conflicts={}\n", s.model.conflict_count()));
                out.push_str(&format!("unresolved={}\n", s.unresolved.len()));
                out.push_str(&format!("hasDifferences={}\n", snap.has_differences()));
            }
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// ⑨ SideBySide：双栏并排（文本；dir/table 为列式清单）
// ---------------------------------------------------------------------------

pub struct SideBySideRenderer;

impl ReportRenderer for SideBySideRenderer {
    fn layout(&self) -> ReportLayout {
        ReportLayout::SideBySide
    }

    fn render(&self, snap: &ReportSnapshot, opts: &ReportOptions) -> Result<String, ReportError> {
        match snap {
            ReportSnapshot::Text(s) => {
                // 列宽 = 可见左行最长（上限 60），确定性截断
                const MAX_COL: usize = 60;
                let vis = visible_text_sections(s, opts);
                let mut width = 1usize;
                for sec in &vis {
                    for t in lines_of(s, crate::options::DiffSide::Left, sec) {
                        width = width.max(t.chars().count().min(MAX_COL));
                    }
                }
                let mut out = String::new();
                let num_w = if opts.line_numbers { 5usize } else { 0 };
                let pad = |t: &str| -> String {
                    let n = t.chars().count();
                    if n >= MAX_COL {
                        format!("{}…", t.chars().take(MAX_COL.saturating_sub(1)).collect::<String>())
                    } else {
                        format!("{t}{}", " ".repeat(width - n))
                    }
                };
                for sec in &vis {
                    let class = row_class(sec.kind, sec.important, opts.ignore_unimportant);
                    let lefts = lines_of(s, crate::options::DiffSide::Left, sec);
                    let rights = lines_of(s, crate::options::DiffSide::Right, sec);
                    let emit = |out: &mut String, lt: Option<&str>, ln: Option<u32>, rt: Option<&str>, rn: Option<u32>| {
                        let lnum = if opts.line_numbers {
                            format!("{:>width$} ", ln.map(|v| v.to_string()).unwrap_or_default(), width = num_w - 1)
                        } else {
                            String::new()
                        };
                        let rnum = if opts.line_numbers {
                            format!("{:>width$} ", rn.map(|v| v.to_string()).unwrap_or_default(), width = num_w - 1)
                        } else {
                            String::new()
                        };
                        let mark = match class {
                            "del" => "-",
                            "ins" => "+",
                            "chg" => "*",
                            _ => " ",
                        };
                        let lpart = match lt {
                            Some(t) => pad(t),
                            None => " ".repeat(width),
                        };
                        let rpart = rt.unwrap_or("");
                        out.push_str(&format!("{lnum}{mark}{lpart} | {rnum}{mark}{rpart}\n"));
                    };
                    match sec.kind {
                        SectionKind::Equal => {
                            for (i, t) in lefts.iter().enumerate() {
                                let no = sec.left.map(|r| r.start + i as u32 + 1);
                                emit(&mut out, Some(t), no, Some(t), no);
                            }
                        }
                        SectionKind::Change => {
                            let max = lefts.len().max(rights.len());
                            for i in 0..max {
                                let ln = sec.left.filter(|_| i < lefts.len()).map(|r| r.start + i as u32 + 1);
                                let rn = sec.right.filter(|_| i < rights.len()).map(|r| r.start + i as u32 + 1);
                                emit(&mut out, lefts.get(i).map(String::as_str), ln, rights.get(i).map(String::as_str), rn);
                            }
                        }
                        SectionKind::Delete => {
                            for (i, t) in lefts.iter().enumerate() {
                                let no = sec.left.map(|r| r.start + i as u32 + 1);
                                emit(&mut out, Some(t), no, None, None);
                            }
                        }
                        SectionKind::Insert => {
                            for (i, t) in rights.iter().enumerate() {
                                let no = sec.right.map(|r| r.start + i as u32 + 1);
                                emit(&mut out, None, None, Some(t), no);
                            }
                        }
                    }
                }
                Ok(out)
            }
            ReportSnapshot::Dir(s) => {
                let mut out = String::new();
                out.push_str(&format!(
                    "{:<40} {:<12} {:>12} {:>12}\n",
                    "路径", "状态", "左大小", "右大小"
                ));
                for row in &s.rows {
                    if !opts.dir_display.keeps(row) {
                        continue;
                    }
                    out.push_str(&format!(
                        "{:<40} {:<12} {:>12} {:>12}\n",
                        truncate_pad(&row.display_path(), 40),
                        row.status.as_str(),
                        row.left.as_ref().map(|i| i.size.to_string()).unwrap_or_else(|| "-".into()),
                        row.right.as_ref().map(|i| i.size.to_string()).unwrap_or_else(|| "-".into()),
                    ));
                }
                Ok(out)
            }
            _ => Err(unsupported(ReportLayout::SideBySide, snap)),
        }
    }
}

fn truncate_pad(s: &str, w: usize) -> String {
    let n = s.chars().count();
    if n >= w {
        let mut t: String = s.chars().take(w.saturating_sub(1)).collect();
        t.push('…');
        t
    } else {
        format!("{s}{}", " ".repeat(w - n))
    }
}

// ---------------------------------------------------------------------------
// ⑩ Interleaved：全量交错（unified 前缀，无 hunk 裁剪）
// ---------------------------------------------------------------------------

pub struct InterleavedRenderer;

impl ReportRenderer for InterleavedRenderer {
    fn layout(&self) -> ReportLayout {
        ReportLayout::Interleaved
    }

    fn render(&self, snap: &ReportSnapshot, opts: &ReportOptions) -> Result<String, ReportError> {
        let s = match snap {
            ReportSnapshot::Text(s) => s,
            _ => return Err(unsupported(ReportLayout::Interleaved, snap)),
        };
        let mut out = String::new();
        out.push_str(&format!("--- {}\n", s.left_label));
        out.push_str(&format!("+++ {}\n", s.right_label));
        for sec in visible_text_sections(s, opts) {
            let mark = |k: SectionKind| match k {
                SectionKind::Equal => ' ',
                SectionKind::Insert => '+',
                SectionKind::Delete => '-',
                SectionKind::Change => '*',
            };
            let m = mark(sec.kind);
            for t in lines_of(s, crate::options::DiffSide::Left, sec) {
                out.push_str(&format!("{m} {t}\n"));
            }
            // Change：右侧行以 + 续（左侧行已按 * 输出）
            if sec.kind == SectionKind::Change {
                for t in lines_of(s, crate::options::DiffSide::Right, sec) {
                    out.push_str(&format!("+ {t}\n"));
                }
            }
            // Insert 只走右侧行；Delete 只走左侧行（上循环已覆盖）
            if sec.kind == SectionKind::Insert {
                // 已在上循环输出（左为空）；无操作
            }
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// 文件名 / 时间戳模板（BC 风格 %f %t 等占位符）
// ---------------------------------------------------------------------------

/// 模板命名上下文（时间由调用方注入保证确定性）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportNaming {
    /// 当前时间（epoch 毫秒）
    pub now_ms: u64,
    /// 左侧基名（无路径）
    pub left_name: String,
    /// 右侧基名
    pub right_name: String,
    /// 会话种类（%k：dir/text/table/hex/merge）
    pub kind: String,
}

/// epoch 毫秒 -> (Y, M, D, h, m, s)（UTC；civil-from-days 算法，无外部依赖）
pub fn civil_from_epoch_ms(ms: u64) -> (i64, u32, u32, u32, u32, u32) {
    let secs = (ms / 1000) as i64;
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let h = (rem / 3600) as u32;
    let mi = ((rem % 3600) / 60) as u32;
    let s = (rem % 60) as u32;
    // Howard Hinnant civil_from_days
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097); // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d, h, mi, s)
}

/// 展开文件名模板占位符：
/// `%f` 左基名 / `%F` 右基名 / `%k` 种类 / `%t` yyyy-mm-dd_hh-mm-ss /
/// `%T` yyyy-mm-dd / `%Y %m %d %H %M %S` 分量 / `%%` 字面 %。
pub fn expand_template(template: &str, naming: &ReportNaming) -> String {
    let (y, mo, d, h, mi, s) = civil_from_epoch_ms(naming.now_ms);
    let mut out = String::with_capacity(template.len() + 16);
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('f') => out.push_str(&naming.left_name),
            Some('F') => out.push_str(&naming.right_name),
            Some('k') => out.push_str(&naming.kind),
            Some('t') => out.push_str(&format!("{y:04}-{mo:02}-{d:02}_{h:02}-{mi:02}-{s:02}")),
            Some('T') => out.push_str(&format!("{y:04}-{mo:02}-{d:02}")),
            Some('Y') => out.push_str(&format!("{y:04}")),
            Some('m') => out.push_str(&format!("{mo:02}")),
            Some('d') => out.push_str(&format!("{d:02}")),
            Some('H') => out.push_str(&format!("{h:02}")),
            Some('M') => out.push_str(&format!("{mi:02}")),
            Some('S') => out.push_str(&format!("{s:02}")),
            Some('%') => out.push('%'),
            Some(other) => {
                out.push('%');
                out.push(other);
            }
            None => out.push('%'),
        }
    }
    out
}

/// 缺省报告文件名："<kind>-report_<t>.<ext>"（如 folder-report_2026-11-14_08-30-00.html）
pub fn default_report_name(naming: &ReportNaming, ext: &str) -> String {
    expand_template("%k-report_%t", naming) + "." + ext
}

/// 布局对应的缺省扩展名
pub fn layout_extension(layout: ReportLayout) -> &'static str {
    match layout {
        ReportLayout::Html => "html",
        ReportLayout::Xml => "xml",
        ReportLayout::Csv => "csv",
        _ => "txt",
    }
}

// ---------------------------------------------------------------------------
// 公共小工具
// ---------------------------------------------------------------------------

fn section_kind_str(k: SectionKind) -> &'static str {
    match k {
        SectionKind::Equal => "equal",
        SectionKind::Insert => "insert",
        SectionKind::Delete => "delete",
        SectionKind::Change => "change",
    }
}

fn hex_block_kind_str(k: HexBlockKind) -> &'static str {
    match k {
        HexBlockKind::Same => "same",
        HexBlockKind::Changed => "changed",
        HexBlockKind::Inserted => "inserted",
        HexBlockKind::Deleted => "deleted",
    }
}

fn merge_origin_str(o: ChunkOrigin) -> &'static str {
    match o {
        ChunkOrigin::Stable => "stable",
        ChunkOrigin::LeftOnly => "leftOnly",
        ChunkOrigin::RightOnly => "rightOnly",
        ChunkOrigin::BothChanged => "bothChanged",
        ChunkOrigin::Conflict => "conflict",
    }
}

// ---------------------------------------------------------------------------
// 单测（渲染矩阵抽查 + HTML 结构 + 模板确定性）
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hex::{diff_bytes, HexOptions};
    use crate::merge::{merge3, MergeOptions};
    use crate::options::DiffSide;
    use crate::table::{diff_tables, TableDiffOptions};
    use crate::{diff_text, LineTable, TextDiffOptions};

    fn text_snap(a: &str, b: &str) -> TextSnapshot {
        let lt = LineTable::from_text(a);
        let rt = LineTable::from_text(b);
        let out = diff_text(&lt, &rt, &TextDiffOptions::default()).unwrap();
        let left_lines: Vec<String> = (0..lt.len()).map(|i| lt.line(i).text.clone()).collect();
        let right_lines: Vec<String> = (0..rt.len()).map(|i| rt.line(i).text.clone()).collect();
        TextSnapshot::new("a.txt", "b.txt", left_lines, right_lines, out.sections)
    }

    fn dir_snap() -> DirSnapshot {
        DirSnapshot {
            left_label: "L".into(),
            right_label: "R".into(),
            rows: vec![
                DirReportRow {
                    rel_path: "same.txt".into(),
                    name: "same.txt".into(),
                    kind: ReportEntryKind::File,
                    status: ReportEntryStatus::Same,
                    left: Some(ReportSideInfo::new(10, 1000)),
                    right: Some(ReportSideInfo::new(10, 1000)),
                },
                DirReportRow {
                    rel_path: "changed.txt".into(),
                    name: "changed.txt".into(),
                    kind: ReportEntryKind::File,
                    status: ReportEntryStatus::ContentDiff,
                    left: Some(ReportSideInfo::new(20, 2000)),
                    right: Some(ReportSideInfo::new(24, 3000)),
                },
                DirReportRow {
                    rel_path: "only_l.txt".into(),
                    name: "only_l.txt".into(),
                    kind: ReportEntryKind::File,
                    status: ReportEntryStatus::LeftOnly,
                    left: Some(ReportSideInfo::new(5, 1500)),
                    right: None,
                },
                DirReportRow {
                    rel_path: "sub".into(),
                    name: "sub".into(),
                    kind: ReportEntryKind::Dir,
                    status: ReportEntryStatus::DirDiff,
                    left: Some(ReportSideInfo::new(0, 0)),
                    right: Some(ReportSideInfo::new(0, 0)),
                },
                DirReportRow {
                    rel_path: "sub/inner.txt".into(),
                    name: "inner.txt".into(),
                    kind: ReportEntryKind::File,
                    status: ReportEntryStatus::ContentDiff,
                    left: Some(ReportSideInfo::new(9, 99)),
                    right: Some(ReportSideInfo::new(10, 100)),
                },
            ],
            stats: DirReportStats {
                total: 5,
                same: 1,
                content_diff: 2,
                dir_diff: 1,
                left_only: 1,
                ..Default::default()
            },
        }
    }

    const SAMPLE_A: &str = "alpha\nbeta\ngamma\ndelta\n";
    const SAMPLE_B: &str = "alpha\nBETA\ngamma\ndelta\nepsilon\n";

    #[test]
    fn summary_layout_lists_sections() {
        let snap = ReportSnapshot::Text(text_snap(SAMPLE_A, SAMPLE_B));
        let out = render_report(&snap, &ReportOptions::default()).unwrap();
        assert!(out.contains("差异清单"));
        assert!(out.contains("[change] 左 2-2 / 右 2-2"), "实际:\n{out}");
        assert!(out.contains("[insert] 左 - / 右 5-5"));
        assert!(out.contains("< beta"), "首行预览:\n{out}");
        assert!(out.contains("> epsilon"));
        // matches 模式：无差异清单
        let opts = ReportOptions { display: DisplayFilter::Matches, ..Default::default() };
        let out = render_report(&snap, &opts).unwrap();
        assert!(!out.contains("差异清单"));
    }

    #[test]
    fn patch_unified_matches_diff_u_shape() {
        let snap = ReportSnapshot::Text(text_snap(SAMPLE_A, SAMPLE_B));
        let opts = ReportOptions { layout: ReportLayout::Patch, ..Default::default() };
        let out = render_report(&snap, &opts).unwrap();
        assert!(out.starts_with("--- a.txt\n+++ b.txt\n"));
        assert!(out.contains("@@ -1,4 +1,5 @@"), "实际:\n{out}");
        assert!(out.contains("-beta\n+BETA\n gamma\n delta\n+epsilon\n"), "实际:\n{out}");
    }

    #[test]
    fn patch_normal_and_context() {
        let snap = ReportSnapshot::Text(text_snap(SAMPLE_A, SAMPLE_B));
        let opts = ReportOptions {
            layout: ReportLayout::Patch,
            patch_style: PatchStyle::Normal,
            ..Default::default()
        };
        let out = render_report(&snap, &opts).unwrap();
        assert!(out.contains("2c2"), "normal 头: {out}");
        assert!(out.contains("< beta"));
        assert!(out.contains("> BETA"));
        assert!(out.contains("4a5"), "追加段: {out}");

        let opts = ReportOptions {
            layout: ReportLayout::Patch,
            patch_style: PatchStyle::Context,
            ..Default::default()
        };
        let out = render_report(&snap, &opts).unwrap();
        assert!(out.contains("*** a.txt"));
        assert!(out.contains("--- b.txt"));
        assert!(out.contains("! beta"));
        assert!(out.contains("! BETA"));
    }

    #[test]
    fn patch_identical_is_empty() {
        let snap = ReportSnapshot::Text(text_snap("x\n", "x\n"));
        let opts = ReportOptions { layout: ReportLayout::Patch, ..Default::default() };
        assert_eq!(render_report(&snap, &opts).unwrap(), "");
    }

    #[test]
    fn html_is_self_contained_and_escaped() {
        let snap = ReportSnapshot::Text(text_snap("a<b>&\"x\"\n", "a<b>&\"y\"\n"));
        let opts = ReportOptions { layout: ReportLayout::Html, ..Default::default() };
        let out = render_report(&snap, &opts).unwrap();
        assert!(out.starts_with("<!DOCTYPE html>"));
        assert!(out.contains("<style>"), "样式内联");
        assert!(!out.contains("<style src"), "无外部样式表");
        assert!(out.contains("&lt;b&gt;&amp;&quot;x&quot;"), "内容转义");
        assert!(out.contains("tr class=\"chg\""));
        // 标签配对
        assert_eq!(out.matches("<table").count(), out.matches("</table>").count());
        assert_eq!(out.matches("<tr").count(), out.matches("</tr>").count());
        assert!(out.trim_end().ends_with("</html>"));
    }

    #[test]
    fn html_dir_orphan_and_links() {
        let snap = ReportSnapshot::Dir(dir_snap());
        let opts = ReportOptions {
            layout: ReportLayout::Html,
            include_file_links: true,
            ..Default::default()
        };
        let out = render_report(&snap, &opts).unwrap();
        assert!(out.contains("<a href=\"changed.txt.html\">changed.txt</a>"));
        assert!(out.contains("orphan"));
        // 孤项不出链接
        assert!(!out.contains("only_l.txt.html"));
    }

    #[test]
    fn xml_structure_roundtrip_escape() {
        let snap = ReportSnapshot::Text(text_snap("<t>\n", "&>\n"));
        let opts = ReportOptions { layout: ReportLayout::Xml, ..Default::default() };
        let out = render_report(&snap, &opts).unwrap();
        assert!(out.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
        assert!(out.contains("&lt;t&gt;"));
        assert!(out.contains("&amp;&gt;"));
        assert_eq!(out.matches("<section ").count(), out.matches("</section>").count());
        let snap = ReportSnapshot::Dir(dir_snap());
        let out = render_report(&snap, &opts).unwrap();
        assert!(out.contains("status=\"contentDiff\""));
        assert!(out.contains("path=\"sub/inner.txt\""));
    }

    #[test]
    fn csv_escapes_and_tsv_delimiter() {
        let snap = ReportSnapshot::Text(text_snap("a,b\n", "a\tb\n"));
        let opts = ReportOptions { layout: ReportLayout::Csv, ..Default::default() };
        let out = render_report(&snap, &opts).unwrap();
        assert!(out.contains("\"a,b\""), "含分隔符需引号: {out}");
        let opts = ReportOptions { layout: ReportLayout::Csv, delimiter: '\t', ..Default::default() };
        let out = render_report(&snap, &opts).unwrap();
        assert!(out.contains("\"a\tb\""));
        assert!(out.lines().next().unwrap().contains("leftStart"));
    }

    #[test]
    fn tree_layout_indents_by_depth() {
        let snap = ReportSnapshot::Dir(dir_snap());
        let opts = ReportOptions { layout: ReportLayout::Tree, ..Default::default() };
        let out = render_report(&snap, &opts).unwrap();
        assert!(out.contains("[=] same.txt"));
        assert!(out.contains("[!] changed.txt"));
        assert!(out.contains("[<] only_l.txt"));
        assert!(out.contains("  [!] inner.txt"), "二级缩进:\n{out}");
        assert!(out.contains("[!] sub/"));
        // dir-only
        let t = ReportSnapshot::Text(text_snap("a\n", "b\n"));
        assert!(render_report(&t, &opts).is_err());
    }

    #[test]
    fn stats_layout_has_no_content() {
        let snap = ReportSnapshot::Text(text_snap(SAMPLE_A, SAMPLE_B));
        let opts = ReportOptions { layout: ReportLayout::Stats, ..Default::default() };
        let out = render_report(&snap, &opts).unwrap();
        assert!(out.contains("insertedLines=2"));
        assert!(out.contains("deletedLines=1"));
        assert!(out.contains("hasDifferences=true"));
        assert!(!out.contains("beta"), "统计布局不含内容");
        // 相同文本
        let snap = ReportSnapshot::Text(text_snap("x\n", "x\n"));
        let out = render_report(&snap, &opts).unwrap();
        assert!(out.contains("hasDifferences=false"));
    }

    #[test]
    fn display_filters() {
        let snap = ReportSnapshot::Text(text_snap("k\n1\n2\n3\n4\n5\nX\n6\n7\n8\n9\nz\n", "k\n1\n2\n3\n4\n5\nY\n6\n7\n8\n9\nz\n"));
        // mismatches：只剩差异
        let opts = ReportOptions { display: DisplayFilter::Mismatches, layout: ReportLayout::Xml, ..Default::default() };
        let out = render_report(&snap, &opts).unwrap();
        assert_eq!(out.matches("<section ").count(), 1, "实际:\n{out}");
        // context=1：差异 + 邻近 1 行
        let opts = ReportOptions { display: DisplayFilter::Context, context_lines: 1, layout: ReportLayout::Xml, ..Default::default() };
        let out = render_report(&snap, &opts).unwrap();
        let secs = out.matches("<section ").count();
        assert!((2..=3).contains(&secs), "context=1 应裁剪: {secs}");
        // matches
        let opts = ReportOptions { display: DisplayFilter::Matches, layout: ReportLayout::Xml, ..Default::default() };
        let out = render_report(&snap, &opts).unwrap();
        assert!(!out.contains("kind=\"change\""));
    }

    #[test]
    fn ignore_unimportant_hides_sections() {
        // 构造带 Unimportant 的快照（直接改 importance）
        let mut snap = text_snap("x\ny\n", "x\nY\n");
        for s in snap.sections.iter_mut() {
            if s.kind != SectionKind::Equal {
                s.important = Importance::Unimportant;
            }
        }
        snap.unimportant_sections = 1;
        let s = ReportSnapshot::Text(snap);
        let opts = ReportOptions { layout: ReportLayout::Patch, ignore_unimportant: true, ..Default::default() };
        assert_eq!(render_report(&s, &opts).unwrap(), "", "全不重要 -> patch 空");
        let opts = ReportOptions { layout: ReportLayout::Stats, ignore_unimportant: true, ..Default::default() };
        let out = render_report(&s, &opts).unwrap();
        assert!(out.contains("unimportantSections=1"));
    }

    #[test]
    fn side_by_side_and_interleaved() {
        let snap = ReportSnapshot::Text(text_snap("one\ntwo\n", "one\nTWO\n"));
        let opts = ReportOptions { layout: ReportLayout::SideBySide, line_numbers: true, ..Default::default() };
        let out = render_report(&snap, &opts).unwrap();
        assert!(out.contains("one"), "{out}");
        assert!(out.contains("*two"), "{out}");
        assert!(out.contains("*TWO"), "{out}");
        // 双栏分隔
        assert!(out.contains(" | "));
        let opts = ReportOptions { layout: ReportLayout::Interleaved, ..Default::default() };
        let out = render_report(&snap, &opts).unwrap();
        assert!(out.contains("--- a.txt"));
        assert!(out.contains("+++ b.txt"));
        assert!(out.contains("* two"));
        assert!(out.contains("+ TWO"));
    }

    #[test]
    fn dir_display_filters() {
        let snap = ReportSnapshot::Dir(dir_snap());
        let base = ReportOptions { layout: ReportLayout::List, ..Default::default() };
        let out = render_report(&snap, &base).unwrap();
        assert_eq!(out.lines().count(), 6, "标题+4 差异行(目录行+子行): \n{out}");
        let opts = ReportOptions { layout: ReportLayout::List, dir_display: DirDisplayFilter::Orphans, ..Default::default() };
        let out = render_report(&snap, &opts).unwrap();
        assert!(out.contains("only_l.txt"));
        assert!(!out.contains("changed.txt"));
        let opts = ReportOptions { layout: ReportLayout::List, dir_display: DirDisplayFilter::MismatchesNoOrphans, ..Default::default() };
        let out = render_report(&snap, &opts).unwrap();
        assert!(out.contains("changed.txt"));
        assert!(!out.contains("only_l.txt"));
    }

    #[test]
    fn table_and_hex_snapshots_render() {
        let model = diff_tables("id,v\n1,a\n2,b\n", "id,v\n1,a\n2,B\n3,c\n", &TableDiffOptions::default()).unwrap();
        let snap = ReportSnapshot::Table(TableSnapshot {
            left_label: "l.csv".into(),
            right_label: "r.csv".into(),
            columns: model.columns.clone(),
            rows: model.rows.clone(),
            summary: model.summary,
            has_header: true,
        });
        let opts = ReportOptions { layout: ReportLayout::Csv, ..Default::default() };
        let out = render_report(&snap, &opts).unwrap();
        assert!(out.contains("added,,2,,3,,c"), "added 行 CSV 形状: {out}");
        assert!(out.contains("changed,1,1,2,2,b,B"));
        let opts = ReportOptions { layout: ReportLayout::Html, ..Default::default() };
        assert!(render_report(&snap, &opts).unwrap().contains("table-diff"));

        let hex_model = diff_bytes(b"AAAABBBB", b"AAAAXXBB", &HexOptions::default());
        let (sl, sr) = crate::hex::status_maps(&hex_model);
        let snap = ReportSnapshot::Hex(HexSnapshot {
            left_label: "l.bin".into(),
            right_label: "r.bin".into(),
            left: b"AAAABBBB".to_vec(),
            right: b"AAAAXXBB".to_vec(),
            model: hex_model,
            status_left: sl,
            status_right: sr,
        });
        for layout in [ReportLayout::Xml, ReportLayout::Html, ReportLayout::Csv, ReportLayout::Stats, ReportLayout::List] {
            let opts = ReportOptions { layout, ..Default::default() };
            let out = render_report(&snap, &opts).unwrap();
            assert!(!out.is_empty(), "{layout:?} 应有输出");
        }
        let opts = ReportOptions { layout: ReportLayout::Html, ..Default::default() };
        let out = render_report(&snap, &opts).unwrap();
        assert!(out.contains("hc"), "字节级着色类");
    }

    #[test]
    fn merge_snapshot_renders() {
        let base = LineTable::from_text("a\nb\nc\n");
        let left = LineTable::from_text("a\nB\nc\n");
        let right = LineTable::from_text("a\nC\nc\n");
        let model = merge3(&base, &left, &right, &MergeOptions::default());
        let unresolved = model.unresolved();
        let snap = ReportSnapshot::Merge(MergeSnapshot {
            base_label: "base".into(),
            left_label: "l".into(),
            right_label: "r".into(),
            model,
            unresolved: unresolved.clone(),
        });
        let opts = ReportOptions { layout: ReportLayout::Xml, ..Default::default() };
        let out = render_report(&snap, &opts).unwrap();
        assert!(out.contains("origin=\"conflict\""));
        assert!(out.contains("unresolved=\"true\""));
        let opts = ReportOptions { layout: ReportLayout::Stats, ..Default::default() };
        let out = render_report(&snap, &opts).unwrap();
        assert!(out.contains(&format!("unresolved={}", unresolved.len())));
    }

    #[test]
    fn patch_roundtrip_with_patch_engine() {
        // P11 roundtrip 基石：report patch 布局输出可被 ai-core::patch 解析应用还原
        let orig = "alpha\nbeta\ngamma\ndelta\n";
        let modified = "alpha\nBETA\ngamma\ndelta\nepsilon\n";
        let snap = ReportSnapshot::Text(text_snap(orig, modified));
        let opts = ReportOptions { layout: ReportLayout::Patch, context_lines: 3, ..Default::default() };
        let patch_text = render_report(&snap, &opts).unwrap();
        let parsed = crate::patch::parse_patch(&patch_text).expect("补丁可解析");
        assert_eq!(parsed.files.len(), 1);
        let applied = crate::patch::apply_file(orig, &parsed.files[0], &crate::patch::PatchApplyOptions::default());
        match applied.unwrap() {
            crate::patch::ApplyOutcome::Applied { text, .. } => assert_eq!(text, modified),
            other => panic!("应用失败: {other:?}"),
        }
    }

    #[test]
    fn templates_expand_deterministically() {
        let naming = ReportNaming {
            now_ms: 1_699_900_000_000, // 2023-11-13 20:40:00 UTC
            left_name: "a.txt".into(),
            right_name: "b.txt".into(),
            kind: "folder".into(),
        };
        assert_eq!(expand_template("%f vs %F", &naming), "a.txt vs b.txt");
        assert_eq!(expand_template("%k_%t", &naming), "folder_2023-11-13_18-26-40");
        assert_eq!(expand_template("%T/%Y%m%d_%H%M%S", &naming), "2023-11-13/20231113_182640");
        assert_eq!(expand_template("100%%", &naming), "100%");
        assert_eq!(expand_template("%z", &naming), "%z", "未知占位符保留");
        assert_eq!(default_report_name(&naming, "html"), "folder-report_2023-11-13_18-26-40.html");
    }

    #[test]
    fn layout_parse_and_extension() {
        for (s, want) in [
            ("summary", ReportLayout::Summary),
            ("patch", ReportLayout::Patch),
            ("html", ReportLayout::Html),
            ("xml", ReportLayout::Xml),
            ("csv", ReportLayout::Csv),
            ("tree", ReportLayout::Tree),
            ("list", ReportLayout::List),
            ("stats", ReportLayout::Stats),
            ("side-by-side", ReportLayout::SideBySide),
            ("interleaved", ReportLayout::Interleaved),
        ] {
            assert_eq!(ReportLayout::parse(s), Some(want));
        }
        assert_eq!(ReportLayout::parse("nope"), None);
        assert_eq!(layout_extension(ReportLayout::Html), "html");
    }

    #[test]
    fn renderer_trait_dispatch_all_layouts() {
        let snap = ReportSnapshot::Dir(dir_snap());
        for layout in [
            ReportLayout::Summary, ReportLayout::Tree, ReportLayout::List,
            ReportLayout::Stats, ReportLayout::Html, ReportLayout::Xml, ReportLayout::Csv,
            ReportLayout::SideBySide,
        ] {
            let r = renderer_for_layout(layout);
            assert_eq!(r.layout(), layout);
            assert!(r.render(&snap, &ReportOptions { layout, ..Default::default() }).is_ok(), "{layout:?}");
        }
        // tree 不支持文本快照
        let t = ReportSnapshot::Text(text_snap("a\n", "b\n"));
        assert!(renderer_for_layout(ReportLayout::Tree)
            .render(&t, &ReportOptions::default())
            .is_err());
    }

    #[test]
    fn dir_row_helpers() {
        let rows = dir_snap().rows;
        assert_eq!(rows[1].display_path(), "changed.txt");
        assert_eq!(rows[3].display_path(), "sub/");
        assert_eq!(rows[4].depth(), 1);
        assert!(status_is_diff(ReportEntryStatus::ContentDiff));
        assert!(status_is_orphan(ReportEntryStatus::RightOnly));
    }

    #[test]
    fn lines_of_respects_ranges() {
        let snap = text_snap("1\n2\n3\n", "1\nX\n3\n");
        let sec = &snap.sections[1];
        assert_eq!(lines_of(&snap, DiffSide::Left, sec), vec!["2".to_string()]);
        assert_eq!(lines_of(&snap, DiffSide::Right, sec), vec!["X".to_string()]);
        let sec0 = &snap.sections[0];
        assert_eq!(lines_of(&snap, DiffSide::Left, sec0), vec!["1".to_string()]);
    }

    #[test]
    fn context_patch_right_shorter_no_panic_and_right_coords() {
        // P1-10 回归：右文件短于左（前置删除使 Equal 段左右起点不等）
        let snap = ReportSnapshot::Text(text_snap("a\nb\n", "b\n"));
        for (style, mark_a) in [
            (PatchStyle::Unified, "-a"),
            (PatchStyle::Context, "! a"),
            (PatchStyle::Normal, "< a"),
        ] {
            let opts = ReportOptions { layout: ReportLayout::Patch, patch_style: style, ..Default::default() };
            let out = render_report(&snap, &opts).expect("右短于左不得 panic");
            assert!(out.contains(mark_a), "{style:?}: 期望含 \"{mark_a}\": {out}");
            if style != PatchStyle::Normal {
                // 右块上下文用真实右下标（b），不再越界/错位（normal 无上下文行）
                assert!(out.contains('b'), "{style:?}: {out}");
            }
        }
    }

    #[test]
    fn ignore_unimportant_patch_keeps_all_lines() {
        // P2-29 回归：折叠不重要差异时 Insert 不丢行、坐标自洽
        let a = "one\nTWO\nthree\nfour\n";
        let mut snap = text_snap(a, "one\n2\nthree\nfour\nfive\n");
        for sec in snap.sections.iter_mut() {
            if sec.kind != SectionKind::Equal {
                sec.important = Importance::Unimportant;
            }
        }
        snap.unimportant_sections = 2;
        let s = ReportSnapshot::Text(snap);
        let opts = ReportOptions { layout: ReportLayout::Patch, ignore_unimportant: true, ..Default::default() };
        let out = render_report(&s, &opts).unwrap();
        // 全部差异均不重要 -> patch 为空（不记入补丁）
        assert_eq!(out, "");
        // 部分重要：折叠段保留行且重要段正确
        let mut snap = text_snap(a, "one\n2\nthree\nfour\nfive\n");
        let mut unimp = 0u32;
        for sec in snap.sections.iter_mut() {
            if sec.kind != SectionKind::Equal {
                sec.important = if unimp == 0 {
                    unimp += 1;
                    Importance::Unimportant
                } else {
                    Importance::Important
                };
            }
        }
        snap.unimportant_sections = 1;
        let s = ReportSnapshot::Text(snap);
        let out = render_report(&s, &opts).unwrap();
        // 折叠段：不重要变更不记入（右行作上下文 " 2"）；重要段（+five）照常记录
        assert!(out.contains("+five"), "重要段照常记录: {out}");
        assert!(out.contains(" 2\n"), "折叠段右行作上下文: {out}");
        assert!(!out.contains("-TWO"), "不重要变更不记入: {out}");
    }

    #[test]
    fn context_patch_numbers_zero_count() {
        // 顶部插入：normal 用 0a1
        let snap = ReportSnapshot::Text(text_snap("b\n", "a\nb\n"));
        let opts = ReportOptions {
            layout: ReportLayout::Patch,
            patch_style: PatchStyle::Normal,
            ..Default::default()
        };
        let out = render_report(&snap, &opts).unwrap();
        assert!(out.contains("0a1"), "{out}");
        assert!(out.contains("> a"));
        // 尾部删除：normal 用 2d1
        let snap = ReportSnapshot::Text(text_snap("a\nb\n", "a\n"));
        let out = render_report(&snap, &opts).unwrap();
        assert!(out.contains("2d1"), "{out}");
        assert!(out.contains("< b"));
    }
}
