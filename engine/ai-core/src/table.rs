//! 表格比较引擎（P9 / FR-TBL-001~006 引擎侧子集；docs/03 §3.1 L2 规则：
//! 只面对"文本 -> 表格数据"的纯函数，无 IO）。
//!
//! 能力面：
//! - RFC4180 CSV/TSV 解析（引号嵌套/转义/换行内嵌；BOM 剥离；分隔符与
//!   文本限定符可配；FR-TBL-005 表格文件格式的类型层）；
//! - 列模型：列名（表头行可配）+ 宽松类型推断（Text/Number/Date）；
//! - 列对齐（FR-TBL-003）：未对齐 / 按左名 / 按右名；列名相似度匹配
//!   （精确 -> 大小写折叠 -> 空白规整，三级递进）；
//! - 行对齐（FR-TBL-004）：键列匹配（可配键列集，键重复按出现序配对）
//!   -> 未命中区域按行序回退（Myers 键对齐）+ 偏斜容差补配；
//!   无键时直接行序对齐（Myers）或位置对齐（不对齐）；
//! - 单元格级 diff：重要/不重要语义复用 P1 重要性键思想（空白三类/大小写；
//!   数值容差为 P10 预留：数值列两值相差 ≤ 容差判不重要）；
//! - 输出表格 section 模型：行类型 same/changed/added/removed + 单元格状态。

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::myers::{diff_seq_deadline, RawOp};
use crate::options::WhitespaceMode;

/// 无键 Standard 路径全表 Myers 行对齐的时间预算。行键无公共子序列时
/// Myers 退化为 O(N²)，以时间兜底：超时降级为粗粒度全删全插（合法脚本），
/// 避免冻结引擎 worker（与 hex FULL_REFINE_BUDGET_MS 同语义）。
pub const MYERS_ROWS_BUDGET_MS: u64 = 1000;

// ---------------------------------------------------------------------------
// 解析（FR-TBL-005 / RFC4180）
// ---------------------------------------------------------------------------

/// 表格解析选项
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableParseOptions {
    /// 列分隔符（CSV 默认 ','；TSV '\t'）
    pub delimiter: char,
    /// 文本限定符（RFC4180 默认 '"'）
    pub quote_char: char,
    /// 首行为表头（FR-TBL-003 表头行配置；true 时 header 取首行，数据从第二行起）
    pub has_header: bool,
}

impl Default for TableParseOptions {
    fn default() -> Self {
        TableParseOptions {
            delimiter: ',',
            quote_char: '"',
            has_header: true,
        }
    }
}

/// 解析错误（宽松解析器仅严格性失败时报错：未闭合引号）
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TableParseError {
    #[error("第 {line} 行：文本限定符未闭合")]
    UnclosedQuote { line: usize },
}

/// 解析出的表格数据（header 为空表示无表头；行内单元格数可不等——松散行）
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TableData {
    pub header: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

impl TableData {
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// 全表最大列数（表头与数据行取最大；松散行短列视为空）
    pub fn width(&self) -> usize {
        self.rows
            .iter()
            .map(|r| r.len())
            .max()
            .unwrap_or(0)
            .max(self.header.len())
    }

    /// 取第 row 行第 col 列单元格（越界 = 空串；松散行补齐语义）
    pub fn cell(&self, row: usize, col: usize) -> &str {
        self.rows
            .get(row)
            .and_then(|r| r.get(col))
            .map(String::as_str)
            .unwrap_or("")
    }

    /// 列名（col 越界 = 生成名 col+1；表头缺失时生成 "Column N"）
    pub fn column_name(&self, col: usize) -> String {
        if let Some(h) = self.header.get(col) {
            if !h.is_empty() {
                return h.clone();
            }
        }
        format!("Column {}", col + 1)
    }
}

/// RFC4180 CSV/TSV 解析（宽松容错）：
/// - 引号字段：限定符内分隔符/换行（含 CRLF/CR/LF）均为字面内容；`""` 转义为一个限定符；
/// - 未引号字段：读到分隔符或行尾；字段内引号按字面保留；
/// - 行尾：CRLF / LF / 裸 CR 均为记录分隔；末行无换行符照常成行；
///   文件以换行结尾不产生幽灵空行；
/// - 真空白行（无分隔符且无内容）跳过（表格比较语义：空行是噪声，
///   FR-TBL-004 "整理删除空行"；含分隔符的空字段行照常成记录）；
/// - BOM（U+FEFF）剥离；限定符后出现杂散字符按字面附加（宽松）。
pub fn parse_table(text: &str, opts: &TableParseOptions) -> Result<TableData, TableParseError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let chars: Vec<char> = text.chars().collect();
    let mut records: Vec<Vec<String>> = Vec::new();
    let mut field = String::new();
    let mut record: Vec<String> = Vec::new();
    let mut in_quotes = false;
    let mut field_quoted = false;
    let mut had_content = false; // 本记录出现过分隔符/字段内容（区分真空白行）
    let mut line = 1usize;
    let mut i = 0usize;

    let d = opts.delimiter;
    let q = opts.quote_char;

    while i < chars.len() {
        let c = chars[i];
        if in_quotes {
            if c == q {
                if chars.get(i + 1) == Some(&q) {
                    field.push(q);
                    i += 2;
                } else {
                    in_quotes = false;
                    i += 1;
                    // 限定符后杂散字符（宽松）：继续按未引号字段读
                }
            } else {
                if c == '\n' {
                    line += 1;
                }
                field.push(c);
                i += 1;
            }
            continue;
        }
        if c == q && field.is_empty() && !field_quoted {
            // 字段起始引号
            in_quotes = true;
            field_quoted = true;
            had_content = true;
            i += 1;
            continue;
        }
        if c == d {
            record.push(std::mem::take(&mut field));
            field_quoted = false;
            had_content = true;
            i += 1;
            continue;
        }
        if c == '\n' || c == '\r' {
            record.push(std::mem::take(&mut field));
            field_quoted = false;
            // CRLF 合并
            if c == '\r' && chars.get(i + 1) == Some(&'\n') {
                i += 2;
            } else {
                i += 1;
            }
            line += 1;
            // 真空白行（无分隔符且无内容）：跳过
            if had_content {
                records.push(std::mem::take(&mut record));
            } else {
                record.clear();
            }
            had_content = false;
            continue;
        }
        field.push(c);
        had_content = true;
        i += 1;
    }
    if in_quotes {
        return Err(TableParseError::UnclosedQuote { line });
    }
    // 末记录（无行尾）：出现过分隔符或字段内容即成行；纯空尾不算
    if had_content || !record.is_empty() {
        record.push(field);
        records.push(record);
    }

    let mut data = TableData::default();
    if opts.has_header {
        if let Some(h) = records.first() {
            data.header = h.clone();
        }
        if !records.is_empty() {
            records.remove(0);
        }
    }
    data.rows = records;
    Ok(data)
}

// ---------------------------------------------------------------------------
// 列模型：类型推断（宽松）+ 列对齐（FR-TBL-003）
// ---------------------------------------------------------------------------

/// 列类型（FR-TBL-003 "常规"自动确定的宽松子集；Date/数值容差细则 P10）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnType {
    Text,
    Number,
    Date,
}

impl ColumnType {
    pub fn as_str(self) -> &'static str {
        match self {
            ColumnType::Text => "text",
            ColumnType::Number => "number",
            ColumnType::Date => "date",
        }
    }
}

/// 宽松数值：含至少一个 ASCII 数字且可完整解析为有限 f64（拒绝 "inf"/"NaN"/空）
pub fn parse_number(s: &str) -> Option<f64> {
    let t = s.trim();
    if t.is_empty() || !t.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    t.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// 宽松日期：`YYYY-MM-DD` / `YYYY/MM/DD` / `MM/DD/YYYY` / `DD.MM.YYYY`（数字合法性粗校验）
pub fn parse_date(s: &str) -> bool {
    let t = s.trim();
    let is_num = |x: &str| !x.is_empty() && x.len() <= 4 && x.chars().all(|c| c.is_ascii_digit());
    let valid = |m: u32, d: u32| (1..=12).contains(&m) && (1..=31).contains(&d);
    // YYYY-MM-DD
    if t.len() == 10 {
        let b = t.as_bytes();
        if b[4] == b'-' && b[7] == b'-' {
            let (y, m, d) = (&t[..4], &t[5..7], &t[8..]);
            if is_num(y) && is_num(m) && is_num(d) {
                return valid(m.parse().unwrap_or(0), d.parse().unwrap_or(0));
            }
        }
    }
    // YYYY/MM/DD 与 MM/DD/YYYY 共用 '/'
    let parts: Vec<&str> = t.split('/').collect();
    if parts.len() == 3 && parts.iter().all(|p| is_num(p)) {
        let p0: u32 = parts[0].parse().unwrap_or(0);
        let p1: u32 = parts[1].parse().unwrap_or(0);
        let p2: u32 = parts[2].parse().unwrap_or(0);
        if parts[0].len() == 4 {
            return valid(p1, p2); // Y/M/D
        }
        if parts[2].len() == 4 {
            return valid(p0, p1); // M/D/Y
        }
    }
    // DD.MM.YYYY
    let parts: Vec<&str> = t.split('.').collect();
    if parts.len() == 3 && parts.iter().all(|p| is_num(p)) && parts[2].len() == 4 {
        let d: u32 = parts[0].parse().unwrap_or(0);
        let m: u32 = parts[1].parse().unwrap_or(0);
        let _y: u32 = parts[2].parse().unwrap_or(0);
        return valid(m, d);
    }
    false
}

/// 对一列的样本值推断类型（宽松：空值不破坏推断；全空 -> Text）
pub fn infer_column_type(values: impl Iterator<Item = String>) -> ColumnType {
    let mut all_number = true;
    let mut all_date = true;
    let mut seen = false;
    for v in values {
        if v.trim().is_empty() {
            continue;
        }
        seen = true;
        if parse_number(&v).is_none() {
            all_number = false;
        }
        if !parse_date(&v) {
            all_date = false;
        }
        if !all_number && !all_date {
            return ColumnType::Text;
        }
    }
    if !seen {
        return ColumnType::Text;
    }
    if all_number {
        ColumnType::Number
    } else if all_date {
        ColumnType::Date
    } else {
        ColumnType::Text
    }
}

/// 列对齐模式（FR-TBL-003）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColumnAlign {
    /// 未对齐（两侧文件顺序）
    #[default]
    None,
    /// 按左侧名称对齐
    ByLeftName,
    /// 按右侧名称对齐
    ByRightName,
}

/// 比较列（显示"比较列"而非物理列：FR-TBL-001）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComparisonColumn {
    pub name: String,
    pub ty: ColumnType,
    /// 左侧物理列下标（None = 该列仅右侧存在）
    pub left: Option<usize>,
    /// 右侧物理列下标
    pub right: Option<usize>,
}

/// 列名相似度三级递进匹配：精确 -> 大小写折叠 -> 空白规整（FR-TBL-003 列名相似度）
fn name_similarity_key(name: &str) -> String {
    name.trim().to_lowercase()
}

#[allow(clippy::needless_range_loop)] // 配对下标逻辑：索引循环即意图
fn align_by_name(
    primary: &TableData,
    secondary: &TableData,
    by_left: bool,
) -> Vec<ComparisonColumn> {
    let pw = primary.width();
    let sw = secondary.width();
    let mut used_sec: Vec<bool> = vec![false; sw];
    let mut matched: Vec<Option<usize>> = vec![None; pw]; // primary col -> secondary col
                                                          // 两轮匹配：第一轮精确；第二轮大小写+空白规整（列名相似度）
    for pass in 0..2 {
        for pc in 0..pw {
            if matched[pc].is_some() {
                continue;
            }
            let pname = primary.column_name(pc);
            let pkey = if pass == 0 {
                pname.clone()
            } else {
                name_similarity_key(&pname)
            };
            for sc in 0..sw {
                if used_sec[sc] {
                    continue;
                }
                let sname = secondary.column_name(sc);
                let skey = if pass == 0 {
                    sname.clone()
                } else {
                    name_similarity_key(&sname)
                };
                if skey == pkey {
                    used_sec[sc] = true;
                    matched[pc] = Some(sc);
                    break;
                }
            }
        }
    }
    // 主侧列序输出（配对与单侧混排，保持主侧顺序），次侧未匹配列尾随
    let mut columns = Vec::with_capacity(pw + sw);
    for pc in 0..pw {
        let name = primary.column_name(pc);
        match matched[pc] {
            Some(sc) => columns.push(ComparisonColumn {
                name,
                ty: ColumnType::Text,
                left: if by_left { Some(pc) } else { Some(sc) },
                right: if by_left { Some(sc) } else { Some(pc) },
            }),
            None => columns.push(ComparisonColumn {
                name,
                ty: ColumnType::Text,
                left: if by_left { Some(pc) } else { None },
                right: if by_left { None } else { Some(pc) },
            }),
        }
    }
    for sc in 0..sw {
        if !used_sec[sc] {
            columns.push(ComparisonColumn {
                name: secondary.column_name(sc),
                ty: ColumnType::Text,
                left: if by_left { None } else { Some(sc) },
                right: if by_left { Some(sc) } else { None },
            });
        }
    }
    columns
}

/// 列对齐（FR-TBL-003）。None = 位置对齐；ByLeftName/ByRightName = 名称匹配
/// （未命中列成为单侧比较列，保持两侧列序）。
pub fn align_columns(
    left: &TableData,
    right: &TableData,
    align: ColumnAlign,
) -> Vec<ComparisonColumn> {
    let mut columns = match align {
        ColumnAlign::None => {
            let width = left.width().max(right.width());
            (0..width)
                .map(|c| ComparisonColumn {
                    name: left.column_name(c),
                    ty: ColumnType::Text,
                    left: (c < left.width()).then_some(c),
                    right: (c < right.width()).then_some(c),
                })
                .collect::<Vec<_>>()
        }
        ColumnAlign::ByLeftName => align_by_name(left, right, true),
        ColumnAlign::ByRightName => align_by_name(right, left, false),
    };
    // 类型推断（宽松）：Number/Date/Text；两侧取样本
    for col in &mut columns {
        let mut samples = Vec::new();
        if let Some(li) = col.left {
            samples.extend((0..left.row_count()).map(|r| left.cell(r, li).to_string()));
        }
        if let Some(ri) = col.right {
            samples.extend((0..right.row_count()).map(|r| right.cell(r, ri).to_string()));
        }
        col.ty = infer_column_type(samples.into_iter());
    }
    columns
}

// ---------------------------------------------------------------------------
// 单元格 diff（重要性语义复用 P1 双键思想；FR-TBL-003 列处理子集）
// ---------------------------------------------------------------------------

/// 单元格重要性选项（FR-TBL-003 列处理：文本列 大小写/空白不重要；数值列容差 P10）
#[derive(Debug, Clone, PartialEq)]
pub struct TableImportance {
    pub case_unimportant: bool,
    /// 空白语义（复用 P1 三类：有效/忽略数量/忽略全部）
    pub whitespace: WhitespaceMode,
    /// 数值容差（P10 预留；Some(tol) 时数值列两值相差 ≤ tol 判不重要）
    pub numeric_tolerance: Option<f64>,
}

impl Default for TableImportance {
    fn default() -> Self {
        TableImportance {
            case_unimportant: false,
            whitespace: WhitespaceMode::Significant,
            numeric_tolerance: None,
        }
    }
}

/// 单元格比较结果
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellDiffStatus {
    Same,
    UnimportantDiff,
    ImportantDiff,
    /// 仅该列左侧有值（右列为单侧列）
    LeftOnly,
    /// 仅右侧有值
    RightOnly,
}

impl CellDiffStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            CellDiffStatus::Same => "same",
            CellDiffStatus::UnimportantDiff => "unimportantDiff",
            CellDiffStatus::ImportantDiff => "importantDiff",
            CellDiffStatus::LeftOnly => "leftOnly",
            CellDiffStatus::RightOnly => "rightOnly",
        }
    }
}

/// 单元格比较键（空白折叠/大小写折叠后；数值列按数值比较）
fn cell_importance_key(value: &str, ty: ColumnType, imp: &TableImportance) -> String {
    if ty == ColumnType::Number {
        if let Some(v) = parse_number(value) {
            return format!("{v}");
        }
    }
    let folded = match imp.whitespace {
        WhitespaceMode::Significant => value.to_string(),
        WhitespaceMode::IgnoreAll => value.chars().filter(|c| !c.is_whitespace()).collect(),
        WhitespaceMode::IgnoreAmount => {
            let mut out = String::with_capacity(value.len());
            let mut in_ws = false;
            for c in value.chars() {
                if c.is_whitespace() {
                    if !in_ws {
                        out.push(' ');
                        in_ws = true;
                    }
                } else {
                    out.push(c);
                    in_ws = false;
                }
            }
            out
        }
    };
    if imp.case_unimportant {
        folded.to_lowercase()
    } else {
        folded
    }
}

/// 单行单元格比较（FR-TBL-003 数值容差：数值列两值均可解析且相差 ≤ 容差 -> 不重要）
pub fn compare_cell(l: &str, r: &str, ty: ColumnType, imp: &TableImportance) -> CellDiffStatus {
    if l == r {
        return CellDiffStatus::Same;
    }
    if ty == ColumnType::Number {
        if let (Some(lv), Some(rv)) = (parse_number(l), parse_number(r)) {
            if let Some(tol) = imp.numeric_tolerance {
                if (lv - rv).abs() <= tol {
                    return CellDiffStatus::UnimportantDiff;
                }
            }
            return CellDiffStatus::ImportantDiff;
        }
    }
    if cell_importance_key(l, ty, imp) == cell_importance_key(r, ty, imp) {
        CellDiffStatus::UnimportantDiff
    } else {
        CellDiffStatus::ImportantDiff
    }
}

// ---------------------------------------------------------------------------
// 行对齐与 diff 模型（FR-TBL-004）
// ---------------------------------------------------------------------------

/// 行对齐算法（FR-TBL-004 子集：不对齐 / 标准（Myers 键对齐）；键列出现时恒走键匹配）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RowAlign {
    /// 不对齐：按行号位置配对（多余一侧为 added/removed）
    #[default]
    Unaligned,
    /// 标准对齐：Myers O(ND) 在行比较键上对齐
    Standard,
}

/// 行类别
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableRowKind {
    Same,
    Changed,
    Added,
    Removed,
}

impl TableRowKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TableRowKind::Same => "same",
            TableRowKind::Changed => "changed",
            TableRowKind::Added => "added",
            TableRowKind::Removed => "removed",
        }
    }
}

/// 单元格 diff（比较列视角）
#[derive(Debug, Clone, PartialEq)]
pub struct CellDiff {
    pub left: Option<String>,
    pub right: Option<String>,
    pub status: CellDiffStatus,
}

/// diff 输出行
#[derive(Debug, Clone, PartialEq)]
pub struct TableDiffRow {
    pub kind: TableRowKind,
    /// 左侧数据行号（0 起；Added 时 None）
    pub left: Option<u32>,
    /// 右侧数据行号
    pub right: Option<u32>,
    /// 逐比较列的单元格 diff
    pub cells: Vec<CellDiff>,
    /// 行整体重要性（任一重要单元格差异 -> true；FR-TBL-001 沟槽红/蓝点数据源）
    pub important: bool,
}

/// diff 统计
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TableDiffSummary {
    pub same_rows: u32,
    pub changed_rows: u32,
    pub added_rows: u32,
    pub removed_rows: u32,
    /// 不重要差异行数（changed 且 !important）
    pub unimportant_rows: u32,
}

/// 表格 diff 完整输出（比较列模型 + 行序列 + 统计）
#[derive(Debug, Clone, PartialEq)]
pub struct TableDiffModel {
    pub columns: Vec<ComparisonColumn>,
    pub rows: Vec<TableDiffRow>,
    pub summary: TableDiffSummary,
}

/// 表格 diff 选项
#[derive(Debug, Clone, PartialEq)]
pub struct TableDiffOptions {
    pub parse: TableParseOptions,
    /// 键列集（FR-TBL-003 键列；物理列下标，0 起；空 = 无键）
    pub key_columns: Vec<usize>,
    pub column_align: ColumnAlign,
    pub row_align: RowAlign,
    /// 偏斜容差（FR-TBL-004）：键未命中回退时按内容补配的最大行距离
    /// （默认 5；0 = 完全不允许偏斜补配）
    pub skew_tolerance: u32,
    pub importance: TableImportance,
}

impl Default for TableDiffOptions {
    fn default() -> Self {
        TableDiffOptions {
            parse: TableParseOptions::default(),
            key_columns: Vec::new(),
            column_align: ColumnAlign::default(),
            row_align: RowAlign::default(),
            skew_tolerance: DEFAULT_SKEW,
            importance: TableImportance::default(),
        }
    }
}

/// 偏斜容差默认值（FR-TBL-004）
pub const DEFAULT_SKEW: u32 = 5;

/// 表格比较主入口：解析两侧 -> 列对齐 -> 行对齐 -> 单元格 diff。
pub fn diff_tables(
    left_text: &str,
    right_text: &str,
    opts: &TableDiffOptions,
) -> Result<TableDiffModel, TableParseError> {
    let left = parse_table(left_text, &opts.parse)?;
    let right = parse_table(right_text, &opts.parse)?;
    Ok(diff_table_data(&left, &right, opts))
}

/// 已解析表格数据的比较（解析可在外部缓存/复用）
pub fn diff_table_data(
    left: &TableData,
    right: &TableData,
    opts: &TableDiffOptions,
) -> TableDiffModel {
    let deadline = Instant::now() + Duration::from_millis(MYERS_ROWS_BUDGET_MS);
    diff_table_data_with_deadline(left, right, opts, deadline)
}

/// 带时间预算的比较实现（deadline 传入供测试注入已过期时刻验证降级）
fn diff_table_data_with_deadline(
    left: &TableData,
    right: &TableData,
    opts: &TableDiffOptions,
    deadline: Instant,
) -> TableDiffModel {
    let columns = align_columns(left, right, opts.column_align);
    let imp = &opts.importance;

    // 行比较键（对齐骨架用）：比较列视角拼接
    let row_key = |side: &TableData, row: usize, which: Side| -> String {
        let mut key = String::new();
        for (ci, col) in columns.iter().enumerate() {
            let idx = match which {
                Side::Left => col.left,
                Side::Right => col.right,
            };
            let v = idx.map(|i| side.cell(row, i)).unwrap_or("");
            let ty = col.ty;
            if ci > 0 {
                key.push('\u{1}');
            }
            key.push_str(&cell_importance_key(v, ty, imp));
        }
        key
    };

    // 键列键（键匹配用；键列按物理列取值，未命中列取空）
    let key_col_key = |side: &TableData, row: usize, which: Side| -> String {
        let mut key = String::new();
        for (i, &kc) in opts.key_columns.iter().enumerate() {
            if i > 0 {
                key.push('\u{1}');
            }
            let v = match which {
                Side::Left => side
                    .rows
                    .get(row)
                    .and_then(|r| r.get(kc))
                    .map(String::as_str),
                Side::Right => side
                    .rows
                    .get(row)
                    .and_then(|r| r.get(kc))
                    .map(String::as_str),
            }
            .unwrap_or("");
            key.push_str(&cell_importance_key(v, ColumnType::Text, imp));
        }
        key
    };

    let pairs: Vec<Option<usize>> = if !opts.key_columns.is_empty() {
        key_pair_rows(left, right, key_col_key)
    } else {
        vec![None; left.row_count()]
    };

    let mut rows: Vec<TableDiffRow> = Vec::new();

    let push_same_changed = |rows: &mut Vec<TableDiffRow>,
                             left: &TableData,
                             right: &TableData,
                             li: usize,
                             ri: usize,
                             columns: &[ComparisonColumn],
                             imp: &TableImportance,
                             kind_fixed: Option<TableRowKind>| {
        let mut cells = Vec::with_capacity(columns.len());
        let mut important = false;
        for col in columns {
            let lv = col.left.map(|i| left.cell(li, i));
            let rv = col.right.map(|i| right.cell(ri, i));
            let status = match (lv.is_some(), rv.is_some()) {
                (true, true) => compare_cell(lv.unwrap(), rv.unwrap(), col.ty, imp),
                (true, false) => CellDiffStatus::LeftOnly,
                (false, true) => CellDiffStatus::RightOnly,
                (false, false) => CellDiffStatus::Same,
            };
            if matches!(
                status,
                CellDiffStatus::ImportantDiff
                    | CellDiffStatus::LeftOnly
                    | CellDiffStatus::RightOnly
            ) {
                important = true;
            }
            cells.push(CellDiff {
                left: lv.map(str::to_string),
                right: rv.map(str::to_string),
                status,
            });
        }
        // 行类别：任一单元格有差异（含不重要差异）-> Changed；全同 -> Same。
        // important 仅由重要差异决定（FR-TBL-001 沟槽红/蓝点：红 = 有重要差异，
        // 蓝 = 仅有不重要差异）
        let any_diff = cells.iter().any(|c| c.status != CellDiffStatus::Same);
        let kind = kind_fixed.unwrap_or(if any_diff {
            TableRowKind::Changed
        } else {
            TableRowKind::Same
        });
        if kind == TableRowKind::Same {
            important = false;
        }
        rows.push(TableDiffRow {
            kind,
            left: Some(li as u32),
            right: Some(ri as u32),
            cells,
            important,
        });
    };

    if !opts.key_columns.is_empty() {
        // 键匹配路径：锚点间区域回退行序对齐（Myers 内容键）+ 偏斜容差补配
        emit_keyed_rows(
            left,
            right,
            &pairs,
            &columns,
            opts,
            &row_key,
            &mut rows,
            &push_same_changed,
        );
    } else {
        match opts.row_align {
            RowAlign::Unaligned => {
                let n = left.row_count().min(right.row_count());
                for i in 0..n {
                    push_same_changed(&mut rows, left, right, i, i, &columns, imp, None);
                }
                for li in n..left.row_count() {
                    push_single(
                        &mut rows,
                        left,
                        &columns,
                        Side::Left,
                        li,
                        TableRowKind::Removed,
                    );
                }
                for ri in n..right.row_count() {
                    push_single(
                        &mut rows,
                        right,
                        &columns,
                        Side::Right,
                        ri,
                        TableRowKind::Added,
                    );
                }
            }
            RowAlign::Standard => {
                emit_myers_rows(
                    left,
                    right,
                    &row_key,
                    &columns,
                    imp,
                    &mut rows,
                    &push_same_changed,
                    deadline,
                );
            }
        }
    }

    let summary = summarize(&rows);
    TableDiffModel {
        columns,
        rows,
        summary,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Left,
    Right,
}

/// 单侧行（Removed/Added）：单侧单元格 + 对侧空
fn push_single(
    rows: &mut Vec<TableDiffRow>,
    side_data: &TableData,
    columns: &[ComparisonColumn],
    side: Side,
    row: usize,
    kind: TableRowKind,
) {
    let mut cells = Vec::with_capacity(columns.len());
    let mut important = false;
    for col in columns {
        let idx = match side {
            Side::Left => col.left,
            Side::Right => col.right,
        };
        let v = idx.map(|i| side_data.cell(row, i));
        let status = match (side, idx.is_some()) {
            (Side::Left, true) | (Side::Right, true) => CellDiffStatus::ImportantDiff,
            _ => CellDiffStatus::Same,
        };
        if idx.is_some() {
            important = true;
        }
        cells.push(CellDiff {
            left: if side == Side::Left {
                v.map(str::to_string)
            } else {
                None
            },
            right: if side == Side::Right {
                v.map(str::to_string)
            } else {
                None
            },
            status,
        });
    }
    rows.push(TableDiffRow {
        kind,
        left: if side == Side::Left {
            Some(row as u32)
        } else {
            None
        },
        right: if side == Side::Right {
            Some(row as u32)
        } else {
            None
        },
        cells,
        important,
    });
}

/// 键列配对（FR-TBL-003：有键时仅键列数据匹配的行对齐；多键优先级按键列顺序）。
/// 键重复按出现序配对；保持单调（右侧配对下标必须递增，防交叉）。
#[allow(clippy::needless_range_loop)] // 配对下标逻辑：索引循环即意图
fn key_pair_rows(
    left: &TableData,
    right: &TableData,
    key_of: impl Fn(&TableData, usize, Side) -> String,
) -> Vec<Option<usize>> {
    let mut queues: std::collections::HashMap<String, VecDeque<usize>> =
        std::collections::HashMap::new();
    for ri in 0..right.row_count() {
        queues
            .entry(key_of(right, ri, Side::Right))
            .or_default()
            .push_back(ri);
    }
    let mut pairs = vec![None; left.row_count()];
    let mut last_right: isize = -1;
    for li in 0..left.row_count() {
        let key = key_of(left, li, Side::Left);
        if let Some(q) = queues.get_mut(&key) {
            // 单调配对：取队列中第一个 > last_right 的候选
            let pos = q.iter().position(|&ri| ri as isize > last_right);
            if let Some(pos) = pos {
                let ri = q.remove(pos).expect("pos 有效");
                pairs[li] = Some(ri);
                last_right = ri as isize;
            }
        }
    }
    pairs
}

/// 键匹配主路径（FR-TBL-003：有键时仅键列数据匹配的行对齐）：
/// 1. 键列单调配对（锚点）；2. 未命中行按行序回退——仅当内容完全一致且全局距离 ≤
///    偏斜容差时补配（FR-TBL-004 接近匹配的有界近似，补配行恒为 Same；键模式不产生
///    内容 Changed 对——“仅键列数据匹配的行对齐”）；3. 余量行按 left 序 emit：
///    removed 先于同区间 added，锚点前的 added 先行。
#[allow(clippy::too_many_arguments)]
#[allow(clippy::needless_range_loop)] // 配对下标逻辑：索引循环即意图
fn emit_keyed_rows(
    left: &TableData,
    right: &TableData,
    pairs: &[Option<usize>],
    columns: &[ComparisonColumn],
    opts: &TableDiffOptions,
    row_key: &impl Fn(&TableData, usize, Side) -> String,
    rows: &mut Vec<TableDiffRow>,
    push_pair: &impl Fn(
        &mut Vec<TableDiffRow>,
        &TableData,
        &TableData,
        usize,
        usize,
        &[ComparisonColumn],
        &TableImportance,
        Option<TableRowKind>,
    ),
) {
    let imp = opts.importance.clone();
    let skew = opts.skew_tolerance as usize;

    // 1. 内容补配：未命中键的行对中，内容键相等且距离 ≤ skew（单调、贪心最近）
    let mut extra: std::collections::HashMap<usize, usize> = std::collections::HashMap::new(); // left -> right
    {
        let l_left: Vec<usize> = (0..left.row_count())
            .filter(|&i| pairs[i].is_none())
            .collect();
        let r_left: Vec<usize> = (0..right.row_count())
            .filter(|&r| !pairs.contains(&Some(r)))
            .collect();
        let mut used_r: std::collections::HashSet<usize> = std::collections::HashSet::new();
        let mut last_r: isize = -1;
        for &li in &l_left {
            let key = row_key(left, li, Side::Left);
            let mut best: Option<(usize, usize)> = None; // (right idx, dist)
            for &ri in &r_left {
                if used_r.contains(&ri) || (ri as isize) <= last_r {
                    continue;
                }
                if row_key(right, ri, Side::Right) != key {
                    continue;
                }
                let dist = ri.abs_diff(li);
                if dist <= skew && best.is_none_or(|(_, bd)| dist < bd) {
                    best = Some((ri, dist));
                }
            }
            if let Some((ri, _)) = best {
                used_r.insert(ri);
                last_r = ri as isize;
                extra.insert(li, ri);
            }
        }
    }

    let pair_right_of = |i: usize| -> Option<usize> { pairs[i].or(extra.get(&i).copied()) };
    let mut emitted_right: std::collections::HashSet<usize> = std::collections::HashSet::new();
    let mut next_pair_right: Vec<Option<usize>> = vec![None; left.row_count() + 1];
    {
        // next_pair_right[i] = i.. 中第一个配对右侧下标（键对或内容对）
        let mut cur: Option<usize> = None;
        for i in (0..left.row_count()).rev() {
            if let Some(pr) = pairs[i] {
                cur = Some(pr);
            }
            next_pair_right[i] = cur;
        }
    }

    let emit_added_before = |rows: &mut Vec<TableDiffRow>,
                             emitted_right: &mut std::collections::HashSet<usize>,
                             bound: usize| {
        for r in 0..bound.min(right.row_count()) {
            if !emitted_right.contains(&r)
                && !pairs.contains(&Some(r))
                && !extra.values().any(|&v| v == r)
            {
                emitted_right.insert(r);
                push_single(rows, right, columns, Side::Right, r, TableRowKind::Added);
            }
        }
    };

    for li in 0..left.row_count() {
        match pair_right_of(li) {
            Some(pr) => {
                // 锚点/补配行前：flush 右侧下标小于 pr 的未配对行（added 先行）
                emit_added_before(rows, &mut emitted_right, pr);
                emitted_right.insert(pr);
                push_pair(rows, left, right, li, pr, columns, &imp, None);
            }
            None => {
                // 未命中行：removed（其前 flush 右侧小于下一个锚点右侧下标的未配对行）
                if let Some(bound) = next_pair_right[li] {
                    emit_added_before(rows, &mut emitted_right, bound);
                }
                push_single(rows, left, columns, Side::Left, li, TableRowKind::Removed);
            }
        }
    }
    // 尾部剩余右侧未配对行 -> added
    for r in 0..right.row_count() {
        if !emitted_right.contains(&r)
            && !pairs.contains(&Some(r))
            && !extra.values().any(|&v| v == r)
        {
            push_single(rows, right, columns, Side::Right, r, TableRowKind::Added);
        }
    }
}

/// 无键 Standard 路径：全表 Myers 行对齐（Equal -> Same；相邻 Delete+Insert
/// 按位置配成 Changed；余量 Removed/Added）
#[allow(clippy::too_many_arguments)]
fn emit_myers_rows(
    left: &TableData,
    right: &TableData,
    row_key: &impl Fn(&TableData, usize, Side) -> String,
    columns: &[ComparisonColumn],
    imp: &TableImportance,
    rows: &mut Vec<TableDiffRow>,
    push_pair: &impl Fn(
        &mut Vec<TableDiffRow>,
        &TableData,
        &TableData,
        usize,
        usize,
        &[ComparisonColumn],
        &TableImportance,
        Option<TableRowKind>,
    ),
    deadline: Instant,
) {
    let lkeys: Vec<String> = (0..left.row_count())
        .map(|i| row_key(left, i, Side::Left))
        .collect();
    let rkeys: Vec<String> = (0..right.row_count())
        .map(|i| row_key(right, i, Side::Right))
        .collect();
    let (ops, _degraded) = diff_seq_deadline(&lkeys, &rkeys, Some(deadline));
    let mut li = 0usize;
    let mut ri = 0usize;
    let mut k = 0usize;
    while k < ops.len() {
        match ops[k] {
            RawOp::Equal(n) => {
                for _ in 0..n as usize {
                    push_pair(rows, left, right, li, ri, columns, imp, None);
                    li += 1;
                    ri += 1;
                }
                k += 1;
            }
            RawOp::Delete(d) => {
                let ins = match ops.get(k + 1) {
                    Some(RawOp::Insert(n)) => *n,
                    _ => 0,
                };
                let paired = (d as usize).min(ins as usize);
                for _ in 0..paired {
                    push_pair(rows, left, right, li, ri, columns, imp, None);
                    li += 1;
                    ri += 1;
                }
                for _ in paired..d as usize {
                    push_single(rows, left, columns, Side::Left, li, TableRowKind::Removed);
                    li += 1;
                }
                for _ in paired..ins as usize {
                    push_single(rows, right, columns, Side::Right, ri, TableRowKind::Added);
                    ri += 1;
                }
                if ins > 0 {
                    k += 2;
                } else {
                    k += 1;
                }
            }
            RawOp::Insert(n) => {
                for _ in 0..n as usize {
                    push_single(rows, right, columns, Side::Right, ri, TableRowKind::Added);
                    ri += 1;
                }
                k += 1;
            }
        }
    }
}

fn summarize(rows: &[TableDiffRow]) -> TableDiffSummary {
    let mut s = TableDiffSummary::default();
    for r in rows {
        match r.kind {
            TableRowKind::Same => s.same_rows += 1,
            TableRowKind::Changed => {
                s.changed_rows += 1;
                if !r.important {
                    s.unimportant_rows += 1;
                }
            }
            TableRowKind::Added => s.added_rows += 1,
            TableRowKind::Removed => s.removed_rows += 1,
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn csv(text: &str) -> TableData {
        parse_table(text, &TableParseOptions::default()).unwrap()
    }

    // ---- 解析 ----

    #[test]
    fn parse_basic_and_header() {
        let t = csv("a,b,c\n1,2,3\n");
        assert_eq!(t.header, vec!["a", "b", "c"]);
        assert_eq!(t.rows, vec![vec!["1", "2", "3"]]);
    }

    #[test]
    fn parse_quoted_field_with_comma_and_quotes() {
        let t = csv("x,y\n\"a,\"\"b\"\"\",c\n");
        assert_eq!(t.rows, vec![vec!["a,\"b\"", "c"]]);
    }

    #[test]
    fn parse_embedded_newline_in_quotes() {
        let t = csv("a,b\n\"line1\nline2\",z\n");
        assert_eq!(t.rows, vec![vec!["line1\nline2", "z"]]);
    }

    #[test]
    fn parse_bom_stripped() {
        let t = csv("\u{feff}h1,h2\nv1,v2\n");
        assert_eq!(t.header, vec!["h1", "h2"]);
    }

    #[test]
    fn parse_tsv_delimiter() {
        let opts = TableParseOptions {
            delimiter: '\t',
            ..Default::default()
        };
        let t = parse_table("a\tb\n1\t2\n", &opts).unwrap();
        assert_eq!(t.header, vec!["a", "b"]);
        assert_eq!(t.rows, vec![vec!["1", "2"]]);
    }

    #[test]
    fn parse_no_trailing_newline_and_empty_rows() {
        let t = csv("a,b\n1,2\n3,4");
        assert_eq!(t.rows.len(), 2);
        // 空文件：0 行 0 列
        let t = csv("");
        assert!(t.header.is_empty() && t.rows.is_empty());
        // 纯空白行：跳过（表格比较语义）
        let t = csv("\n");
        assert!(t.header.is_empty() && t.rows.is_empty());
        // 中段空白行跳过；含分隔符的空字段行保留
        let t = csv("a,b\n\n1,,3\n");
        assert_eq!(t.rows, vec![vec!["1", "", "3"]]);
        let t = csv("a,b\n,\n");
        assert_eq!(t.rows, vec![vec!["", ""]]);
    }

    #[test]
    fn parse_crlf_and_bare_cr() {
        let t = csv("a,b\r\n1,2\r\n3,4\r5,6");
        assert_eq!(t.rows.len(), 3, "CRLF 两条 + 裸 CR 一条: {t:?}");
    }

    #[test]
    fn parse_unclosed_quote_is_error() {
        let e = parse_table("a,b\n\"open", &TableParseOptions::default()).unwrap_err();
        assert!(matches!(e, TableParseError::UnclosedQuote { .. }));
    }

    #[test]
    fn parse_ragged_rows_preserved() {
        let t = csv("a,b,c\n1\n1,2,3,4\n");
        assert_eq!(t.rows[0], vec!["1"]);
        assert_eq!(t.rows[1], vec!["1", "2", "3", "4"]);
        assert_eq!(t.width(), 4);
        assert_eq!(t.cell(0, 2), "", "松散行短列取空");
    }

    // ---- 类型推断 ----

    #[test]
    fn infer_types_loose() {
        assert_eq!(
            infer_column_type(["1", "2.5", "-3e2"].into_iter().map(String::from)),
            ColumnType::Number
        );
        assert_eq!(
            infer_column_type(["2024-01-02", "2024/3/4"].into_iter().map(String::from)),
            ColumnType::Date
        );
        assert_eq!(
            infer_column_type(["1", "x"].into_iter().map(String::from)),
            ColumnType::Text
        );
        // 空值不破坏推断
        assert_eq!(
            infer_column_type(["", "7"].into_iter().map(String::from)),
            ColumnType::Number
        );
        assert_eq!(
            infer_column_type([""].into_iter().map(String::from)),
            ColumnType::Text
        );
        assert!(!parse_number("NaN").is_some() && parse_number("inf").is_none());
        assert!(parse_number(" 42 ").is_some());
        assert!(parse_date("2024-12-31"));
        assert!(parse_date("3/4/2024"));
        assert!(!parse_date("2024-13-01"));
    }

    // ---- 列对齐 ----

    #[test]
    fn columns_positional_default() {
        let l = csv("a,b\n1,2\n");
        let r = csv("x,y\n3,4\n");
        let cols = align_columns(&l, &r, ColumnAlign::None);
        assert_eq!(cols.len(), 2);
        assert_eq!(cols[0].name, "a");
        assert_eq!((cols[0].left, cols[0].right), (Some(0), Some(0)));
    }

    #[test]
    fn columns_by_left_name_reorders_and_appends_orphan() {
        let l = csv("id,name,qty\n1,a,2\n");
        let r = csv("qty,name\n9,b\n"); // 右侧列序打乱且缺 id
        let cols = align_columns(&l, &r, ColumnAlign::ByLeftName);
        let names: Vec<&str> = cols.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["id", "name", "qty"]);
        assert_eq!(cols[0].left, Some(0));
        assert_eq!(cols[0].right, None, "id 列右侧缺失 -> 单侧列");
        assert_eq!(cols[1].left, Some(1));
        assert_eq!(cols[1].right, Some(1));
        assert_eq!(cols[2].left, Some(2));
        assert_eq!(cols[2].right, Some(0), "qty 按名对齐到右 0 列");
    }

    #[test]
    fn columns_by_left_name_case_and_trim_similarity() {
        let l = csv("ID, Name \n1,2\n");
        let r = csv("id,name\n3,4\n");
        let cols = align_columns(&l, &r, ColumnAlign::ByLeftName);
        assert_eq!(cols[0].right, Some(0), "大小写折叠匹配");
        assert_eq!(cols[1].right, Some(1), "空白规整匹配");
    }

    #[test]
    fn column_type_inferred_in_alignment() {
        let l = csv("id,when\n1,2024-01-02\n");
        let r = csv("id,when\n2,2024-02-03\n");
        let cols = align_columns(&l, &r, ColumnAlign::None);
        assert_eq!(cols[0].ty, ColumnType::Number);
        assert_eq!(cols[1].ty, ColumnType::Date);
    }

    // ---- 单元格比较 ----

    #[test]
    fn cell_compare_case_and_whitespace() {
        let imp = TableImportance {
            case_unimportant: true,
            ..Default::default()
        };
        assert_eq!(
            compare_cell("Abc", "abc", ColumnType::Text, &imp),
            CellDiffStatus::UnimportantDiff
        );
        let imp = TableImportance {
            whitespace: WhitespaceMode::IgnoreAmount,
            ..Default::default()
        };
        assert_eq!(
            compare_cell("a  b", "a b", ColumnType::Text, &imp),
            CellDiffStatus::UnimportantDiff
        );
        assert_eq!(
            compare_cell("a", "b", ColumnType::Text, &TableImportance::default()),
            CellDiffStatus::ImportantDiff
        );
    }

    #[test]
    fn cell_compare_numeric_tolerance() {
        let imp = TableImportance {
            numeric_tolerance: Some(0.5),
            ..Default::default()
        };
        assert_eq!(
            compare_cell("1.0", "1.4", ColumnType::Number, &imp),
            CellDiffStatus::UnimportantDiff
        );
        assert_eq!(
            compare_cell("1.0", "2.0", ColumnType::Number, &imp),
            CellDiffStatus::ImportantDiff
        );
        // 无容差：数值不同即重要
        assert_eq!(
            compare_cell(
                "1.0",
                "1.0001",
                ColumnType::Number,
                &TableImportance::default()
            ),
            CellDiffStatus::ImportantDiff
        );
    }

    // ---- 行对齐与 diff ----

    fn kinds(m: &TableDiffModel) -> Vec<&'static str> {
        m.rows.iter().map(|r| r.kind.as_str()).collect()
    }

    #[test]
    fn diff_key_mode_basic_and_gutter() {
        let l = csv("id,v\n1,a\n2,b\n3,c\n");
        let r = csv("id,v\n1,a\n2,B\n4,d\n");
        let opts = TableDiffOptions {
            key_columns: vec![0],
            ..Default::default()
        };
        let m = diff_table_data(&l, &r, &opts);
        assert_eq!(
            kinds(&m),
            vec!["same", "changed", "removed", "added"],
            "{m:?}"
        );
        assert_eq!(m.rows[1].left, Some(1));
        assert_eq!(m.rows[1].right, Some(1));
        assert!(m.rows[1].important);
        assert_eq!(m.summary.removed_rows, 1);
        assert_eq!(m.summary.added_rows, 1);
    }

    #[test]
    fn diff_key_mode_crossed_keys_paired_by_content_within_skew() {
        // 键交叉（monotone 配对留下一对未命中）：内容一致 + 距离 ≤ skew -> 补配 Same
        let l = csv("id,v\n1,a\n2,b\n");
        let r = csv("id,v\n2,b\n1,a\n");
        let opts = TableDiffOptions {
            key_columns: vec![0],
            ..Default::default()
        };
        let m = diff_table_data(&l, &r, &opts);
        assert_eq!(
            kinds(&m),
            vec!["same", "same"],
            "交叉键补配（距离 1 ≤ 默认 5）: {m:?}"
        );
        // skew=0：无补配 -> removed + added
        let opts0 = TableDiffOptions {
            key_columns: vec![0],
            skew_tolerance: 0,
            ..Default::default()
        };
        let m0 = diff_table_data(&l, &r, &opts0);
        assert_eq!(
            kinds(&m0),
            vec!["added", "same", "removed"],
            "无补配：锚点前右行 added 先行，未命中左行 removed: {m0:?}"
        );
        // 键交叉场景的 skew 边界：每对未命中行的内容等价对距离为 1 ——
        // skew=0 不补配；skew=1 全部补配
        let l2 = csv("id,v\n1,a\n2,b\n3,c\n4,d\n5,e\n");
        let r2 = csv("id,v\n2,b\n3,c\n4,d\n5,e\n1,a\n");
        let m0 = diff_table_data(
            &l2,
            &r2,
            &TableDiffOptions {
                key_columns: vec![0],
                skew_tolerance: 0,
                ..Default::default()
            },
        );
        assert_eq!(
            kinds(&m0),
            vec![
                "added", "added", "added", "added", "same", "removed", "removed", "removed",
                "removed"
            ],
            "skew=0：键单调配对只剩 (1,a)，其余 removed/added: {m0:?}"
        );
        let m1 = diff_table_data(
            &l2,
            &r2,
            &TableDiffOptions {
                key_columns: vec![0],
                skew_tolerance: 1,
                ..Default::default()
            },
        );
        assert_eq!(
            kinds(&m1),
            vec!["same", "same", "same", "same", "same"],
            "skew=1：内容补配全部命中: {m1:?}"
        );
    }

    #[test]
    fn diff_key_mode_insert_before_anchor_flushes_added_first() {
        let l = csv("id,v\n1,a\n2,b\n");
        let r = csv("id,v\n9,z\n1,a\n2,b\n");
        let m = diff_table_data(
            &l,
            &r,
            &TableDiffOptions {
                key_columns: vec![0],
                ..Default::default()
            },
        );
        assert_eq!(
            kinds(&m),
            vec!["added", "same", "same"],
            "锚点前新增行先输出: {m:?}"
        );
    }

    #[test]
    fn diff_key_mode_duplicate_keys_pair_in_order() {
        let l = csv("id,v\n1,a\n1,b\n1,c\n");
        let r = csv("id,v\n1,A\n1,B\n");
        let opts = TableDiffOptions {
            key_columns: vec![0],
            ..Default::default()
        };
        let m = diff_table_data(&l, &r, &opts);
        assert_eq!(
            kinds(&m),
            vec!["changed", "changed", "removed"],
            "键重复按出现序配对: {m:?}"
        );
    }

    #[test]
    fn diff_no_key_standard_alignment() {
        let l = csv("v\na\nb\nc\n");
        let r = csv("v\na\nB\nc\n");
        let m = diff_table_data(&l, &r, &TableDiffOptions::default());
        assert_eq!(kinds(&m), vec!["same", "changed", "same"], "{m:?}");
    }

    #[test]
    fn standard_myers_degrades_on_expired_deadline() {
        // 无键 Standard 全表 Myers 漏接 deadline 会冻结引擎；以时间预算兜底。
        // 右表中部插入一行：最优对齐给出 same/added/same/same（无 changed）；
        // 过期 deadline 降级为全删全插 → 按位置错配出 changed，仍覆盖全部行。
        let l = csv("v\na\nb\nc\n");
        let r = csv("v\na\nX\nb\nc\n");
        let opts = TableDiffOptions {
            row_align: RowAlign::Standard,
            ..Default::default()
        };
        let m = diff_table_data_with_deadline(&l, &r, &opts, Instant::now());
        assert!(
            kinds(&m).contains(&"changed"),
            "过期 deadline 应降级为位置错配（出现 changed）: {m:?}"
        );
        let left_rows = m.rows.iter().filter(|r| r.left.is_some()).count();
        let right_rows = m.rows.iter().filter(|r| r.right.is_some()).count();
        assert_eq!(left_rows, 3, "降级脚本左表每行须有归属");
        assert_eq!(right_rows, 4, "降级脚本右表每行须有归属");
    }

    #[test]
    fn diff_no_key_unaligned_positional() {
        let l = csv("v\na\nb\n");
        let r = csv("v\na\nB\nc\n");
        let opts = TableDiffOptions {
            row_align: RowAlign::Unaligned,
            ..Default::default()
        };
        let m = diff_table_data(&l, &r, &opts);
        assert_eq!(kinds(&m), vec!["same", "changed", "added"], "{m:?}");
    }

    #[test]
    fn diff_importance_marks_unimportant_rows() {
        // 键模式：键列相同、非键列仅不重要方面不同 -> changed 且 important=false（蓝点行）
        let l = csv("id,v\n1,Abc\n");
        let r = csv("id,v\n1,abc \n");
        let opts = TableDiffOptions {
            key_columns: vec![0],
            importance: TableImportance {
                case_unimportant: true,
                whitespace: WhitespaceMode::IgnoreAll,
                ..Default::default()
            },
            ..Default::default()
        };
        let m = diff_table_data(&l, &r, &opts);
        assert_eq!(kinds(&m), vec!["changed"]);
        assert!(!m.rows[0].important);
        assert_eq!(m.summary.unimportant_rows, 1);
        // 默认重要性：同样的差异是重要差异
        let m2 = diff_table_data(
            &l,
            &r,
            &TableDiffOptions {
                key_columns: vec![0],
                ..Default::default()
            },
        );
        assert!(m2.rows[0].important);
        // 无键 Standard：整行键折叠后相同 -> Myers 判等，但单元格仍有不重要差异
        // -> changed + !important（蓝点行，"只显示差异"应包含）
        let m3 = diff_table_data(
            &csv("v\nAbc\n"),
            &csv("v\nabc \n"),
            &TableDiffOptions {
                importance: TableImportance {
                    case_unimportant: true,
                    whitespace: WhitespaceMode::IgnoreAll,
                    ..Default::default()
                },
                ..Default::default()
            },
        );
        assert_eq!(kinds(&m3), vec!["changed"], "仅不重要差异 -> 蓝点行");
        assert!(!m3.rows[0].important);
    }

    #[test]
    fn diff_columns_reordered_by_name_with_cell_values_mapped() {
        let l = csv("id,qty\n1,10\n");
        let r = csv("qty,id\n10,1\n");
        let opts = TableDiffOptions {
            column_align: ColumnAlign::ByLeftName,
            ..Default::default()
        };
        let m = diff_table_data(&l, &r, &opts);
        assert_eq!(kinds(&m), vec!["same"], "列序打乱按名对齐后全同: {m:?}");
        assert_eq!(m.columns[0].name, "id");
        assert_eq!(m.columns[0].right, Some(1));
    }

    #[test]
    fn diff_unicode_and_empty_columns() {
        let l = csv("名,値\n中文,1\n");
        let r = csv("名,値\n中文,\n");
        let m = diff_table_data(&l, &r, &TableDiffOptions::default());
        assert_eq!(kinds(&m), vec!["changed"], "{m:?}");
        assert_eq!(m.rows[0].cells[1].left.as_deref(), Some("1"));
        assert_eq!(m.rows[0].cells[1].right.as_deref(), Some(""));
    }

    #[test]
    fn diff_parse_error_propagates() {
        let e = diff_tables("a,b\n\"x", "a,b\n", &TableDiffOptions::default()).unwrap_err();
        assert!(matches!(e, TableParseError::UnclosedQuote { .. }));
    }

    #[test]
    fn summary_counts() {
        let l = csv("id\n1\n2\n3\n");
        let r = csv("id\n1\n4\n");
        let opts = TableDiffOptions {
            key_columns: vec![0],
            ..Default::default()
        };
        let m = diff_table_data(&l, &r, &opts);
        assert_eq!(m.summary.same_rows, 1);
        assert_eq!(m.summary.changed_rows, 0);
        assert_eq!(m.summary.added_rows, 1);
        assert_eq!(m.summary.removed_rows, 2);
    }
}
