//! 引擎层错误 -> 协议错误码映射；ai-core section -> 协议 Section DTO 映射。

use ai_protocol::error::codes;
use ai_protocol::{Importance, InlineState, LineRange, Section, SectionKind};
use ai_vfs::VfsError;

/// VFS 错误 -> RPC 错误对象（领域码见 docs/03 附录 B / error.rs 约定）
pub fn vfs_error_to_rpc(err: &VfsError) -> ai_protocol::RpcErrorObject {
    let (code, msg) = match err {
        VfsError::NotFound { path } => (codes::PATH_NOT_FOUND, format!("路径不存在: {path}")),
        VfsError::NotADirectory { path } => (codes::PATH_NOT_FOUND, format!("不是目录: {path}")),
        VfsError::NotAFile { path } => (codes::PATH_NOT_FOUND, format!("不是文件: {path}")),
        VfsError::InvalidPath(m) => (codes::INVALID_PARAMS, format!("非法路径: {m}")),
        VfsError::InvalidEncoding { path } => {
            (codes::INVALID_ENCODING, format!("文件不是合法 UTF-8 文本: {path}"))
        }
        VfsError::Cancelled => (codes::REQUEST_CANCELLED, "操作被取消".to_string()),
        VfsError::Unsupported(m) => (codes::INTERNAL_ERROR, format!("不支持的操作: {m}")),
        VfsError::Io(e) => (codes::INTERNAL_ERROR, format!("IO 错误: {e}")),
        // P7 期 ai-vfs 新增变体（并发 agent）：目标已存在 -> 411 会话/目标冲突语义族
        VfsError::AlreadyExists { path } => (codes::SESSION_EXISTS, format!("目标已存在: {path}")),
    };
    ai_protocol::error_object(code, msg)
}

/// 目录扫描错误 -> RPC 错误对象
pub fn dir_scan_error_to_rpc(err: &ai_dir::DirScanError) -> ai_protocol::RpcErrorObject {
    match err {
        ai_dir::DirScanError::RootNotFound { side, path } => ai_protocol::error_object(
            codes::PATH_NOT_FOUND,
            format!("根路径不存在（{side}）: {path}"),
        ),
        ai_dir::DirScanError::Cancelled => {
            ai_protocol::error_object(codes::REQUEST_CANCELLED, "扫描被取消")
        }
        ai_dir::DirScanError::Vfs(e) => vfs_error_to_rpc(e),
    }
}

/// ai-core section -> 协议 Section（字段一一对应；importance P0 恒 Important）
pub fn section_to_dto(s: &ai_core::sections::Section) -> Section {
    Section {
        id: s.id,
        kind: match s.kind {
            ai_core::sections::SectionKind::Equal => SectionKind::Equal,
            ai_core::sections::SectionKind::Insert => SectionKind::Insert,
            ai_core::sections::SectionKind::Delete => SectionKind::Delete,
            ai_core::sections::SectionKind::Change => SectionKind::Change,
        },
        left: s.left.map(line_range_to_dto),
        right: s.right.map(line_range_to_dto),
        important: match s.important {
            ai_core::sections::Importance::Important => Importance::Important,
            ai_core::sections::Importance::Unimportant => Importance::Unimportant,
            ai_core::sections::Importance::Ignored => Importance::Ignored,
        },
        inline: match s.inline {
            ai_core::sections::InlineState::NotComputed => InlineState::NotComputed,
            ai_core::sections::InlineState::Partial => InlineState::Partial,
            ai_core::sections::InlineState::Full => InlineState::Full,
        },
    }
}

fn line_range_to_dto(r: ai_core::sections::LineRange) -> LineRange {
    LineRange { start: r.start, end: r.end }
}

/// 文本选项 DTO -> ai-core 选项
pub fn text_options_from_dto(opts: &ai_protocol::TextDiffOptions) -> ai_core::TextDiffOptions {
    ai_core::TextDiffOptions {
        algo: match opts.algo {
            ai_protocol::LineAlignAlgo::Standard => ai_core::LineAlignAlgo::Standard,
            ai_protocol::LineAlignAlgo::Myers => ai_core::LineAlignAlgo::Myers,
            ai_protocol::LineAlignAlgo::Patience => ai_core::LineAlignAlgo::Patience,
            ai_protocol::LineAlignAlgo::Histogram => ai_core::LineAlignAlgo::Histogram,
        },
        whitespace: match opts.whitespace {
            ai_protocol::WhitespaceMode::Significant => ai_core::WhitespaceMode::Significant,
            ai_protocol::WhitespaceMode::IgnoreAmount => ai_core::WhitespaceMode::IgnoreAmount,
            ai_protocol::WhitespaceMode::IgnoreAll => ai_core::WhitespaceMode::IgnoreAll,
        },
        ignore_case: opts.ignore_case,
        ignore_eol_style: opts.ignore_eol_style,
        inline: ai_core::InlineOptions::default(),
        rules: ai_core::RulesOptions::default(),
    }
}

/// ai-core DiffSummary -> 协议 DiffSummary
pub fn summary_to_dto(s: &ai_core::DiffSummary) -> ai_protocol::DiffSummary {
    ai_protocol::DiffSummary {
        section_count: s.section_count,
        equal_sections: s.equal_sections,
        insert_sections: s.insert_sections,
        delete_sections: s.delete_sections,
        change_sections: s.change_sections,
        equal_lines: s.equal_lines,
        inserted_lines: s.inserted_lines,
        deleted_lines: s.deleted_lines,
    }
}

// ---------------------------------------------------------------------------
// P3：行内差异 / 双键 / 重要性 / 手动对齐 的协议映射
// ---------------------------------------------------------------------------

/// 协议 Side -> core 侧别
pub fn side_from_dto(side: ai_protocol::DiffSide) -> ai_core::options::DiffSide {
    match side {
        ai_protocol::DiffSide::Left => ai_core::options::DiffSide::Left,
        ai_protocol::DiffSide::Right => ai_core::options::DiffSide::Right,
    }
}

/// core 侧别 -> 协议 Side
pub fn side_to_dto(side: ai_core::options::DiffSide) -> ai_protocol::DiffSide {
    match side {
        ai_core::options::DiffSide::Left => ai_protocol::DiffSide::Left,
        ai_core::options::DiffSide::Right => ai_protocol::DiffSide::Right,
    }
}

fn char_range_to_dto(r: ai_core::CharSpan) -> ai_protocol::CharRange {
    ai_protocol::CharRange { start: r.start, end: r.end }
}

/// core 行内区间 -> 协议 InlineSpanDto
pub fn inline_span_to_dto(s: &ai_core::InlineSpan) -> ai_protocol::InlineSpanDto {
    ai_protocol::InlineSpanDto {
        left: s.left.map(char_range_to_dto),
        right: s.right.map(char_range_to_dto),
        kind: match s.kind {
            ai_core::InlineSpanKind::Equal => ai_protocol::InlineSpanKind::Equal,
            ai_core::InlineSpanKind::Replace => ai_protocol::InlineSpanKind::Replace,
            ai_core::InlineSpanKind::Insert => ai_protocol::InlineSpanKind::Insert,
            ai_core::InlineSpanKind::Delete => ai_protocol::InlineSpanKind::Delete,
        },
    }
}

/// core 单行行内结果 -> 协议 InlineLineDiffDto
pub fn inline_line_diff_to_dto(d: &ai_core::InlineLineDiff) -> ai_protocol::InlineLineDiffDto {
    ai_protocol::InlineLineDiffDto {
        left_line: d.left_line,
        right_line: d.right_line,
        spans: d.result.spans.iter().map(inline_span_to_dto).collect(),
        truncated: d.result.truncated,
    }
}

/// 重要性选项 DTO -> core 选项（字段一一对应；缺省值与 core 默认一致）
pub fn importance_from_dto(spec: &ai_protocol::ImportanceOptionsSpec) -> ai_core::ImportanceOptions {
    ai_core::ImportanceOptions {
        leading_ws_unimportant: spec.leading_ws_unimportant,
        embedded_ws_unimportant: spec.embedded_ws_unimportant,
        trailing_ws_unimportant: spec.trailing_ws_unimportant,
        case_unimportant: spec.case_unimportant,
        eol_style_unimportant: spec.eol_style_unimportant,
        encoding_unimportant: spec.encoding_unimportant,
    }
}

/// 手动对齐选项 DTO -> core 选项（偏斜容差 0 由 core effective_skew 取默认）
pub fn manual_from_dto(spec: &ai_protocol::TextManualAlignSpec) -> ai_core::ManualAlignOptions {
    fn side_range(sr: &ai_protocol::SideLineRange) -> (ai_core::options::DiffSide, ai_core::sections::LineRange) {
        (side_from_dto(sr.side), ai_core::sections::LineRange::new(sr.range.start, sr.range.end))
    }
    ai_core::ManualAlignOptions {
        align_anchors: spec
            .align_anchors
            .iter()
            .map(|a| ai_core::AlignAnchor {
                left: ai_core::sections::LineRange::new(a.left.start, a.left.end),
                right: ai_core::sections::LineRange::new(a.right.start, a.right.end),
            })
            .collect(),
        isolate: spec.isolate.iter().map(side_range).collect(),
        never_align_important: spec.never_align_important,
        skew_tolerance: spec.skew_tolerance,
        proximity_match: spec.proximity_match,
        ignore: spec.ignore.iter().map(side_range).collect(),
    }
}

/// 逐行双键条目（side + 全局行号 + raw/importance 键）
pub fn line_keys_entry(
    side: ai_core::options::DiffSide,
    line: u32,
    table: &ai_core::LineTable,
    index: usize,
    opts: &ai_core::TextDiffOptions,
    imp: &ai_core::ImportanceOptions,
) -> ai_protocol::LineKeysEntry {
    let entry = table.line(index);
    ai_protocol::LineKeysEntry {
        side: side_to_dto(side),
        line,
        raw: ai_core::raw_key(entry, opts).into_owned(),
        importance: ai_core::importance_key(entry, opts, imp, Some(index)).into_owned(),
    }
}

// ---------------------------------------------------------------------------
// P10：文件格式与规则体系（formats/*、rules/*）协议映射
// ---------------------------------------------------------------------------

/// core 语法项 -> 协议 GrammarItemSpec
fn syntax_item_to_spec(item: &ai_core::SyntaxItem) -> ai_protocol::GrammarItemSpec {
    use ai_core::SyntaxItem;
    match item {
        SyntaxItem::Text { pattern, regex, case_sensitive } => ai_protocol::GrammarItemSpec {
            kind: ai_protocol::GrammarItemKind::Text,
            pattern: Some(pattern.clone()),
            regex: *regex,
            case_sensitive: *case_sensitive,
            ..Default::default()
        },
        SyntaxItem::Delimited { start, end, escape, to_eol, regex, case_sensitive } => {
            ai_protocol::GrammarItemSpec {
                kind: ai_protocol::GrammarItemKind::Delimited,
                start: Some(start.clone()),
                end: end.clone(),
                escape: escape.clone(),
                to_eol: *to_eol,
                regex: *regex,
                case_sensitive: *case_sensitive,
                ..Default::default()
            }
        }
        SyntaxItem::List { tokens, regex, case_sensitive } => ai_protocol::GrammarItemSpec {
            kind: ai_protocol::GrammarItemKind::List,
            tokens: tokens.clone(),
            regex: *regex,
            case_sensitive: *case_sensitive,
            ..Default::default()
        },
        SyntaxItem::Column { from, to } => ai_protocol::GrammarItemSpec {
            kind: ai_protocol::GrammarItemKind::Column,
            from: Some(*from),
            to: *to,
            ..Default::default()
        },
        SyntaxItem::RowTitle { pattern, lines, or_first_line } => ai_protocol::GrammarItemSpec {
            kind: ai_protocol::GrammarItemKind::RowTitle,
            pattern: Some(pattern.clone()),
            lines: Some(*lines),
            or_first_line: *or_first_line,
            ..Default::default()
        },
    }
}

/// 协议 GrammarItemSpec -> core 语法项
pub fn syntax_item_from_spec(spec: &ai_protocol::GrammarItemSpec) -> ai_core::SyntaxItem {
    use ai_core::SyntaxItem;
    match spec.kind {
        ai_protocol::GrammarItemKind::Text => SyntaxItem::Text {
            pattern: spec.pattern.clone().unwrap_or_default(),
            regex: spec.regex,
            case_sensitive: spec.case_sensitive,
        },
        ai_protocol::GrammarItemKind::Delimited => SyntaxItem::Delimited {
            start: spec.start.clone().unwrap_or_default(),
            end: spec.end.clone(),
            escape: spec.escape.clone(),
            to_eol: spec.to_eol,
            regex: spec.regex,
            case_sensitive: spec.case_sensitive,
        },
        ai_protocol::GrammarItemKind::List => SyntaxItem::List {
            tokens: spec.tokens.clone(),
            regex: spec.regex,
            case_sensitive: spec.case_sensitive,
        },
        ai_protocol::GrammarItemKind::Column => SyntaxItem::Column {
            from: spec.from.unwrap_or(1),
            to: spec.to,
        },
        ai_protocol::GrammarItemKind::RowTitle => SyntaxItem::RowTitle {
            pattern: spec.pattern.clone().unwrap_or_default(),
            lines: spec.lines.unwrap_or(1),
            or_first_line: spec.or_first_line,
        },
    }
}

/// core 文件格式 -> 协议 FileFormatSpec
pub fn format_to_spec(f: &ai_core::FileFormat) -> ai_protocol::FileFormatSpec {
    ai_protocol::FileFormatSpec {
        id: f.id.clone(),
        enabled: f.enabled,
        masks: f.masks.clone(),
        description: f.description.clone(),
        view_kind: match f.view_kind {
            ai_core::FormatViewKind::Text => ai_protocol::FormatViewKind::Text,
            ai_core::FormatViewKind::Table => ai_protocol::FormatViewKind::Table,
            ai_core::FormatViewKind::Hex => ai_protocol::FormatViewKind::Hex,
            ai_core::FormatViewKind::Picture => ai_protocol::FormatViewKind::Picture,
            ai_core::FormatViewKind::External => ai_protocol::FormatViewKind::External,
        },
        grammar: f
            .grammar
            .iter()
            .map(|e| ai_protocol::GrammarElementSpec {
                name: e.name.clone(),
                case_sensitive: e.case_sensitive,
                items: e.items.iter().map(syntax_item_to_spec).collect(),
            })
            .collect(),
        tab_stops: f.tab_stops,
        independent_lines: f.independent_lines,
        column_data: f.column_data,
    }
}

/// 协议 FileFormatSpec -> core 文件格式
pub fn format_from_spec(spec: &ai_protocol::FileFormatSpec) -> ai_core::FileFormat {
    ai_core::FileFormat {
        id: spec.id.clone(),
        enabled: spec.enabled,
        masks: spec.masks.clone(),
        description: spec.description.clone(),
        view_kind: match spec.view_kind {
            ai_protocol::FormatViewKind::Text => ai_core::FormatViewKind::Text,
            ai_protocol::FormatViewKind::Table => ai_core::FormatViewKind::Table,
            ai_protocol::FormatViewKind::Hex => ai_core::FormatViewKind::Hex,
            ai_protocol::FormatViewKind::Picture => ai_core::FormatViewKind::Picture,
            ai_protocol::FormatViewKind::External => ai_core::FormatViewKind::External,
        },
        grammar: spec
            .grammar
            .iter()
            .map(|e| ai_core::GrammarElement {
                name: e.name.clone(),
                case_sensitive: e.case_sensitive,
                items: e.items.iter().map(syntax_item_from_spec).collect(),
            })
            .collect(),
        weights: Vec::new(),
        tab_stops: spec.tab_stops,
        independent_lines: spec.independent_lines,
        column_data: spec.column_data,
        encoding: None,
    }
}

/// core 规则层 -> 协议 RuleLayerSpec
pub fn rule_layer_to_spec(layer: &ai_core::RuleLayer) -> ai_protocol::RuleLayerSpec {
    ai_protocol::RuleLayerSpec {
        compare: layer.compare.as_ref().map(compare_rules_to_spec),
        unimportant_elements: layer.unimportant_elements.clone(),
    }
}

/// 协议 RuleLayerSpec -> core 规则层
pub fn rule_layer_from_spec(spec: &ai_protocol::RuleLayerSpec) -> ai_core::RuleLayer {
    ai_core::RuleLayer {
        compare: spec.compare.as_ref().map(compare_rules_from_spec),
        unimportant_elements: spec.unimportant_elements.clone(),
    }
}

/// core CompareRules（全字段具值）-> 协议 CompareRulesSpec
fn compare_rules_to_spec(c: &ai_core::CompareRules) -> ai_protocol::CompareRulesSpec {
    ai_protocol::CompareRulesSpec {
        leading_ws_unimportant: Some(c.leading_ws_unimportant),
        embedded_ws_unimportant: Some(c.embedded_ws_unimportant),
        trailing_ws_unimportant: Some(c.trailing_ws_unimportant),
        case_unimportant: Some(c.case_unimportant),
        eol_style_unimportant: Some(c.eol_style_unimportant),
        encoding_unimportant: Some(c.encoding_unimportant),
    }
}

/// 协议 CompareRulesSpec（部分）-> core CompareRules（None = 缺省值）
fn compare_rules_from_spec(spec: &ai_protocol::CompareRulesSpec) -> ai_core::CompareRules {
    let d = ai_core::CompareRules::default();
    ai_core::CompareRules {
        leading_ws_unimportant: spec.leading_ws_unimportant.unwrap_or(d.leading_ws_unimportant),
        embedded_ws_unimportant: spec.embedded_ws_unimportant.unwrap_or(d.embedded_ws_unimportant),
        trailing_ws_unimportant: spec.trailing_ws_unimportant.unwrap_or(d.trailing_ws_unimportant),
        case_unimportant: spec.case_unimportant.unwrap_or(d.case_unimportant),
        eol_style_unimportant: spec.eol_style_unimportant.unwrap_or(d.eol_style_unimportant),
        encoding_unimportant: spec.encoding_unimportant.unwrap_or(d.encoding_unimportant),
    }
}

/// core EffectiveRules -> 协议 EffectiveRulesSpec
pub fn effective_rules_to_spec(e: &ai_core::EffectiveRules) -> ai_protocol::EffectiveRulesSpec {
    ai_protocol::EffectiveRulesSpec {
        compare: compare_rules_to_spec(&e.compare),
        unimportant_elements: e.unimportant_elements.clone(),
    }
}

/// 会话 rules 载荷（text/open params.rules）-> (ImportanceOptions, 元素集 Option)。
/// None 字段 = 引擎缺省（ImportanceOptions::default）；元素集 None = 不启用语法折叠。
pub fn rules_spec_to_core(spec: &ai_protocol::RulesSpec) -> (ai_core::ImportanceOptions, Option<Vec<String>>) {
    let d = ai_core::ImportanceOptions::default();
    let imp = ai_core::ImportanceOptions {
        leading_ws_unimportant: spec.leading_ws_unimportant.unwrap_or(d.leading_ws_unimportant),
        embedded_ws_unimportant: spec.embedded_ws_unimportant.unwrap_or(d.embedded_ws_unimportant),
        trailing_ws_unimportant: spec.trailing_ws_unimportant.unwrap_or(d.trailing_ws_unimportant),
        case_unimportant: spec.case_unimportant.unwrap_or(d.case_unimportant),
        eol_style_unimportant: spec.eol_style_unimportant.unwrap_or(d.eol_style_unimportant),
        encoding_unimportant: spec.encoding_unimportant.unwrap_or(d.encoding_unimportant),
    };
    (imp, spec.unimportant_elements.clone())
}

// ---------------------------------------------------------------------------
// P7：三向合并（text/merge3/*）协议映射
// ---------------------------------------------------------------------------

/// text/merge3/open options DTO -> core MergeOptions（缺省与 core default 一致）
pub fn merge_options_from_dto(spec: &ai_protocol::Merge3OptionsSpec) -> ai_core::MergeOptions {
    let mut o = ai_core::MergeOptions::default();
    if let Some(algo) = spec.algo {
        o.diff.algo = match algo {
            ai_protocol::LineAlignAlgo::Standard => ai_core::LineAlignAlgo::Standard,
            ai_protocol::LineAlignAlgo::Myers => ai_core::LineAlignAlgo::Myers,
            ai_protocol::LineAlignAlgo::Patience => ai_core::LineAlignAlgo::Patience,
            ai_protocol::LineAlignAlgo::Histogram => ai_core::LineAlignAlgo::Histogram,
        };
    }
    if let Some(ws) = spec.whitespace {
        o.diff.whitespace = match ws {
            ai_protocol::WhitespaceMode::Significant => ai_core::WhitespaceMode::Significant,
            ai_protocol::WhitespaceMode::IgnoreAmount => ai_core::WhitespaceMode::IgnoreAmount,
            ai_protocol::WhitespaceMode::IgnoreAll => ai_core::WhitespaceMode::IgnoreAll,
        };
    }
    if let Some(b) = spec.ignore_case {
        o.diff.ignore_case = b;
    }
    if let Some(b) = spec.ignore_eol_style {
        o.diff.ignore_eol_style = b;
    }
    o.conflict_window = spec.conflict_window;
    o.favor = spec.favor.map(|f| match f {
        ai_protocol::MergeFavorSide::Left => ai_core::FavorSide::Left,
        ai_protocol::MergeFavorSide::Right => ai_core::FavorSide::Right,
    });
    if let Some(b) = spec.ignore_unimportant {
        o.ignore_unimportant = b;
    }
    if let Some(imp) = &spec.importance {
        o.importance = importance_from_dto(imp);
    }
    if let Some(d) = spec.conflict_default {
        o.conflict_default = match d {
            ai_protocol::MergeConflictDefault::Left => ai_core::ConflictDefault::TakeLeft,
            ai_protocol::MergeConflictDefault::Right => ai_core::ConflictDefault::TakeRight,
            ai_protocol::MergeConflictDefault::Base => ai_core::ConflictDefault::TakeBase,
            ai_protocol::MergeConflictDefault::LeftThenRight => ai_core::ConflictDefault::TakeLeftThenRight,
            ai_protocol::MergeConflictDefault::RightThenLeft => ai_core::ConflictDefault::TakeRightThenLeft,
            ai_protocol::MergeConflictDefault::Marker => ai_core::ConflictDefault::Marker,
        };
    }
    o
}

/// 协议决策 DTO -> core 决策（Manual 需 text）
pub fn merge_decision_from_dto(
    d: &ai_protocol::MergeDecisionDto,
) -> Result<ai_core::MergeDecision, String> {
    use ai_protocol::MergeDecisionKind as K;
    Ok(match d.action {
        K::Left => ai_core::MergeDecision::TakeLeft,
        K::Right => ai_core::MergeDecision::TakeRight,
        K::Base => ai_core::MergeDecision::TakeBase,
        K::LeftThenRight => ai_core::MergeDecision::TakeLeftThenRight,
        K::RightThenLeft => ai_core::MergeDecision::TakeRightThenLeft,
        K::Manual => {
            let text = d.text.as_deref().ok_or("Manual 决策需要 text 字段")?;
            ai_core::MergeDecision::Manual(
                text.split('\n').map(|s| s.to_string()).collect::<Vec<_>>(),
            )
        }
    })
}

/// core 决策 -> 协议决策动作（Manual 决策协议面只报动作，编辑文本以块决策为准）
fn decision_kind_to_dto(d: &ai_core::MergeDecision) -> ai_protocol::MergeDecisionKind {
    use ai_core::MergeDecision as D;
    match d {
        D::TakeLeft => ai_protocol::MergeDecisionKind::Left,
        D::TakeRight => ai_protocol::MergeDecisionKind::Right,
        D::TakeBase => ai_protocol::MergeDecisionKind::Base,
        D::TakeLeftThenRight => ai_protocol::MergeDecisionKind::LeftThenRight,
        D::TakeRightThenLeft => ai_protocol::MergeDecisionKind::RightThenLeft,
        D::Manual(_) => ai_protocol::MergeDecisionKind::Manual,
    }
}

fn line_range_dto(r: ai_core::sections::LineRange) -> ai_protocol::LineRange {
    ai_protocol::LineRange { start: r.start, end: r.end }
}

/// 合并块 -> 协议 MergeChunkDto（三方行内容 + 决策状态 + 当前合成输出行）
pub fn merge_chunk_to_dto(
    c: &ai_core::MergeChunk,
    decisions: &std::collections::HashMap<u32, ai_core::MergeDecision>,
    base: &ai_core::LineTable,
    left: &ai_core::LineTable,
    right: &ai_core::LineTable,
    syn: &ai_core::SynthesizeOptions,
) -> ai_protocol::MergeChunkDto {
    use ai_core::ChunkOrigin as O;
    let decision = decisions.get(&c.id).or(c.auto_decision.as_ref());
    let one = |t: &ai_core::LineTable, r: std::ops::Range<u32>| {
        (r.start..r.end).map(|i| t.line(i as usize).text.clone()).collect::<Vec<_>>()
    };
    // 当前块输出行（决策/策略生效后；marker 行由 output 模式整体渲染，此处为空）
    let output_lines = chunk_output_lines(c, decisions, base, left, right, syn);
    let conflict = c.conflict.as_ref();
    ai_protocol::MergeChunkDto {
        id: c.id,
        origin: match c.origin {
            O::Stable => ai_protocol::MergeChunkOrigin::Stable,
            O::LeftOnly => ai_protocol::MergeChunkOrigin::LeftOnly,
            O::RightOnly => ai_protocol::MergeChunkOrigin::RightOnly,
            O::BothChanged => ai_protocol::MergeChunkOrigin::BothChanged,
            O::Conflict => ai_protocol::MergeChunkOrigin::Conflict,
        },
        base: line_range_dto(c.base),
        left: line_range_dto(c.left),
        right: line_range_dto(c.right),
        conflict_kind: conflict.map(|d| match d.kind {
            ai_core::ConflictKind::ModifyModify => ai_protocol::MergeConflictKind::ModifyModify,
            ai_core::ConflictKind::DeleteModify => ai_protocol::MergeConflictKind::DeleteModify,
            ai_core::ConflictKind::AddAdd => ai_protocol::MergeConflictKind::AddAdd,
        }),
        conflict_rows: conflict.map(|d| {
            d.rows
                .iter()
                .map(|r| ai_protocol::MergeConflictRowDto { left: r.left, right: r.right, unimportant: r.unimportant })
                .collect()
        }),
        conflict_unimportant: conflict.map(|d| d.unimportant),
        base_lines: if c.base.is_empty() { Vec::new() } else { one(base, c.base.start..c.base.end) },
        left_lines: if c.left.is_empty() { Vec::new() } else { one(left, c.left.start..c.left.end) },
        right_lines: if c.right.is_empty() { Vec::new() } else { one(right, c.right.start..c.right.end) },
        decided: decisions.contains_key(&c.id),
        decision: decision.map(decision_kind_to_dto),
        auto_resolved: c.auto_decision.is_some(),
        unresolved: c.origin == O::Conflict && decision.is_none(),
        output_lines,
    }
}

/// 单块输出行（与 core synthesize 决策/策略语义一致；供 sections 与 output 共用）
fn chunk_output_lines(
    c: &ai_core::MergeChunk,
    decisions: &std::collections::HashMap<u32, ai_core::MergeDecision>,
    base: &ai_core::LineTable,
    left: &ai_core::LineTable,
    right: &ai_core::LineTable,
    syn: &ai_core::SynthesizeOptions,
) -> Vec<String> {
    let take = |t: &ai_core::LineTable, r: std::ops::Range<u32>| {
        (r.start..r.end).map(|i| t.line(i as usize).text.clone()).collect::<Vec<_>>()
    };
    let decision = decisions.get(&c.id).or(c.auto_decision.as_ref());
    match decision {
        Some(d) => match d {
            ai_core::MergeDecision::TakeLeft => take(left, c.left.start..c.left.end),
            ai_core::MergeDecision::TakeRight => take(right, c.right.start..c.right.end),
            ai_core::MergeDecision::TakeBase => take(base, c.base.start..c.base.end),
            ai_core::MergeDecision::TakeLeftThenRight => {
                let mut v = take(left, c.left.start..c.left.end);
                v.extend(take(right, c.right.start..c.right.end));
                v
            }
            ai_core::MergeDecision::TakeRightThenLeft => {
                let mut v = take(right, c.right.start..c.right.end);
                v.extend(take(left, c.left.start..c.left.end));
                v
            }
            ai_core::MergeDecision::Manual(lines) => lines.clone(),
        },
        None => match c.origin {
            ai_core::ChunkOrigin::Stable | ai_core::ChunkOrigin::LeftOnly | ai_core::ChunkOrigin::BothChanged => {
                take(left, c.left.start..c.left.end)
            }
            ai_core::ChunkOrigin::RightOnly => take(right, c.right.start..c.right.end),
            ai_core::ChunkOrigin::Conflict => match &syn.unresolved {
                ai_core::UnresolvedPolicy::TakeLeft => take(left, c.left.start..c.left.end),
                ai_core::UnresolvedPolicy::TakeRight => take(right, c.right.start..c.right.end),
                ai_core::UnresolvedPolicy::TakeBase => take(base, c.base.start..c.base.end),
                ai_core::UnresolvedPolicy::TakeLeftThenRight => {
                    let mut v = take(left, c.left.start..c.left.end);
                    v.extend(take(right, c.right.start..c.right.end));
                    v
                }
                ai_core::UnresolvedPolicy::TakeRightThenLeft => {
                    let mut v = take(right, c.right.start..c.right.end);
                    v.extend(take(left, c.left.start..c.left.end));
                    v
                }
                ai_core::UnresolvedPolicy::Markers(_) => Vec::new(), // marker 行由 output 模式渲染
            },
        },
    }
}

// ---------------------------------------------------------------------------
// P9：table/hex/patch/dir-execute DTO 映射
// ---------------------------------------------------------------------------

/// ai-core 比较列 -> 协议 TableColumnInfo
pub fn table_column_to_dto(c: &ai_core::table::ComparisonColumn) -> ai_protocol::TableColumnInfo {
    ai_protocol::TableColumnInfo {
        name: c.name.clone(),
        ty: match c.ty {
            ai_core::table::ColumnType::Text => ai_protocol::TableColumnType::Text,
            ai_core::table::ColumnType::Number => ai_protocol::TableColumnType::Number,
            ai_core::table::ColumnType::Date => ai_protocol::TableColumnType::Date,
        },
        left: c.left.map(|i| i as u32),
        right: c.right.map(|i| i as u32),
    }
}

/// ai-core 统计 -> 协议 TableSummary
pub fn table_summary_to_dto(s: &ai_core::table::TableDiffSummary) -> ai_protocol::TableSummary {
    ai_protocol::TableSummary {
        same_rows: s.same_rows,
        changed_rows: s.changed_rows,
        added_rows: s.added_rows,
        removed_rows: s.removed_rows,
        unimportant_rows: s.unimportant_rows,
    }
}

/// ai-core diff 行 -> 协议 TableRowDto
pub fn table_row_to_dto(r: &ai_core::table::TableDiffRow) -> ai_protocol::TableRowDto {
    ai_protocol::TableRowDto {
        kind: match r.kind {
            ai_core::table::TableRowKind::Same => ai_protocol::TableRowKind::Same,
            ai_core::table::TableRowKind::Changed => ai_protocol::TableRowKind::Changed,
            ai_core::table::TableRowKind::Added => ai_protocol::TableRowKind::Added,
            ai_core::table::TableRowKind::Removed => ai_protocol::TableRowKind::Removed,
        },
        left: r.left,
        right: r.right,
        important: r.important,
        cells: r
            .cells
            .iter()
            .map(|c| ai_protocol::TableCellDto {
                left: c.left.clone(),
                right: c.right.clone(),
                status: match c.status {
                    ai_core::table::CellDiffStatus::Same => ai_protocol::TableCellStatus::Same,
                    ai_core::table::CellDiffStatus::UnimportantDiff => ai_protocol::TableCellStatus::UnimportantDiff,
                    ai_core::table::CellDiffStatus::ImportantDiff => ai_protocol::TableCellStatus::ImportantDiff,
                    ai_core::table::CellDiffStatus::LeftOnly => ai_protocol::TableCellStatus::LeftOnly,
                    ai_core::table::CellDiffStatus::RightOnly => ai_protocol::TableCellStatus::RightOnly,
                },
            })
            .collect(),
    }
}

/// ai-core 补丁报告 -> 协议 PatchApplyResult
pub fn patch_report_to_dto(r: &ai_core::patch::PatchApplyReport) -> ai_protocol::PatchApplyResult {
    let results: Vec<ai_protocol::PatchFileResultDto> = r
        .results
        .iter()
        .map(|f| ai_protocol::PatchFileResultDto {
            path: f.path.clone(),
            status: match f.status {
                ai_core::patch::FileApplyStatus::Applied => ai_protocol::PatchFileStatus::Applied,
                ai_core::patch::FileApplyStatus::AlreadyApplied => ai_protocol::PatchFileStatus::AlreadyApplied,
                ai_core::patch::FileApplyStatus::SkippedBinary => ai_protocol::PatchFileStatus::SkippedBinary,
                ai_core::patch::FileApplyStatus::NoOp => ai_protocol::PatchFileStatus::NoOp,
                ai_core::patch::FileApplyStatus::Failed => ai_protocol::PatchFileStatus::Failed,
            },
            fuzz_used: f.fuzz_used,
            offset_applied: f.offset_applied,
            detail: f.detail.clone(),
        })
        .collect();
    let applied = r.results.iter().filter(|f| matches!(f.status, ai_core::patch::FileApplyStatus::Applied)).count();
    let failed = r.results.iter().filter(|f| matches!(f.status, ai_core::patch::FileApplyStatus::Failed | ai_core::patch::FileApplyStatus::AlreadyApplied)).count();
    ai_protocol::PatchApplyResult {
        total: u32::try_from(r.total).unwrap_or(u32::MAX),
        applied: u32::try_from(applied).unwrap_or(u32::MAX),
        failed: u32::try_from(failed).unwrap_or(u32::MAX),
        dry_run: r.dry_run,
        all_applied: r.all_applied(),
        results,
    }
}

/// 协议 DirExecuteOp -> ai-dir FileOp（同构映射；copy/move 缺省目标 = 对侧）
pub fn dir_execute_op_from_dto(op: &ai_protocol::DirExecuteOp) -> Result<ai_dir::FileOp, String> {
    use ai_dir::execute::Side;
    let side_of = |s: ai_protocol::DiffSide| match s {
        ai_protocol::DiffSide::Left => Side::Left,
        ai_protocol::DiffSide::Right => Side::Right,
    };
    let side = side_of(op.side);
    let opposite = match side {
        Side::Left => Side::Right,
        Side::Right => Side::Left,
    };
    let target = op.target_side.map(side_of).unwrap_or(opposite);
    match op.kind {
        ai_protocol::DirExecuteOpKind::Copy => Ok(ai_dir::FileOp::Copy { path: op.path.clone(), from: side, to: target }),
        ai_protocol::DirExecuteOpKind::Move => Ok(ai_dir::FileOp::Move { path: op.path.clone(), from: side, to: target }),
        ai_protocol::DirExecuteOpKind::Rename => Ok(ai_dir::FileOp::Rename {
            path: op.path.clone(),
            side,
            new_name: op.new_name.clone().ok_or("rename 操作需要 newName 字段")?,
        }),
        ai_protocol::DirExecuteOpKind::Delete => Ok(ai_dir::FileOp::Delete { path: op.path.clone(), side }),
        ai_protocol::DirExecuteOpKind::Touch => Ok(ai_dir::FileOp::Touch { path: op.path.clone(), side, mtime_ms: op.mtime_ms.unwrap_or(0) }),
        ai_protocol::DirExecuteOpKind::Mkdir => Ok(ai_dir::FileOp::Mkdir { path: op.path.clone(), side }),
    }
}

/// ai-dir FileOp -> 协议 DirExecuteOp（逆映射；目标侧缺省为对侧时不输出）
pub fn dir_execute_op_to_dto(op: &ai_dir::FileOp) -> ai_protocol::DirExecuteOp {
    use ai_dir::execute::Side;
    let side_dto = |s: Side| match s {
        Side::Left => ai_protocol::DiffSide::Left,
        Side::Right => ai_protocol::DiffSide::Right,
    };
    let opposite_of = |s: Side| match s {
        Side::Left => Side::Right,
        Side::Right => Side::Left,
    };
    match op {
        ai_dir::FileOp::Copy { path, from, to } => ai_protocol::DirExecuteOp {
            kind: ai_protocol::DirExecuteOpKind::Copy,
            path: path.clone(),
            side: side_dto(*from),
            target_side: (*to != opposite_of(*from)).then(|| side_dto(*to)),
            new_name: None,
            mtime_ms: None,
        },
        ai_dir::FileOp::Move { path, from, to } => ai_protocol::DirExecuteOp {
            kind: ai_protocol::DirExecuteOpKind::Move,
            path: path.clone(),
            side: side_dto(*from),
            target_side: (*to != opposite_of(*from)).then(|| side_dto(*to)),
            new_name: None,
            mtime_ms: None,
        },
        ai_dir::FileOp::Rename { path, side, new_name } => ai_protocol::DirExecuteOp {
            kind: ai_protocol::DirExecuteOpKind::Rename,
            path: path.clone(),
            side: side_dto(*side),
            target_side: None,
            new_name: Some(new_name.clone()),
            mtime_ms: None,
        },
        ai_dir::FileOp::Delete { path, side } => ai_protocol::DirExecuteOp {
            kind: ai_protocol::DirExecuteOpKind::Delete,
            path: path.clone(),
            side: side_dto(*side),
            target_side: None,
            new_name: None,
            mtime_ms: None,
        },
        ai_dir::FileOp::Touch { path, side, mtime_ms } => ai_protocol::DirExecuteOp {
            kind: ai_protocol::DirExecuteOpKind::Touch,
            path: path.clone(),
            side: side_dto(*side),
            target_side: None,
            new_name: None,
            mtime_ms: (*mtime_ms != 0).then_some(*mtime_ms),
        },
        ai_dir::FileOp::Mkdir { path, side } => ai_protocol::DirExecuteOp {
            kind: ai_protocol::DirExecuteOpKind::Mkdir,
            path: path.clone(),
            side: side_dto(*side),
            target_side: None,
            new_name: None,
            mtime_ms: None,
        },
    }
}

/// ai-dir OpResult -> 协议 DirExecuteOpResult（状态由调用方按清单归属标注）
pub fn dir_execute_result_to_dto(
    r: &ai_dir::execute::OpResult,
    status: ai_protocol::DirExecuteOpStatus,
) -> ai_protocol::DirExecuteOpResult {
    ai_protocol::DirExecuteOpResult {
        op: dir_execute_op_to_dto(&r.op),
        status,
        detail: r.detail.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ai_core::sections::SectionKind as CoreKind;

    #[test]
    fn section_mapping_roundtrip_shape() {
        let core = ai_core::sections::Section {
            id: 3,
            kind: CoreKind::Change,
            left: Some(ai_core::sections::LineRange { start: 1, end: 4 }),
            right: Some(ai_core::sections::LineRange { start: 2, end: 6 }),
            important: ai_core::sections::Importance::Important,
            inline: ai_core::sections::InlineState::NotComputed,
        };
        let dto = section_to_dto(&core);
        assert_eq!(dto.id, 3);
        assert_eq!(dto.kind, SectionKind::Change);
        assert_eq!(dto.left.unwrap().start, 1);
        assert_eq!(dto.right.unwrap().end, 6);
        assert_eq!(dto.important, Importance::Important);
    }

    #[test]
    fn vfs_error_codes() {
        let e = vfs_error_to_rpc(&VfsError::NotFound { path: "/x".into() });
        assert_eq!(e.code, codes::PATH_NOT_FOUND);
        let e = vfs_error_to_rpc(&VfsError::InvalidEncoding { path: "/x".into() });
        assert_eq!(e.code, codes::INVALID_ENCODING);
        let e = vfs_error_to_rpc(&VfsError::Io(std::io::Error::other("disk")));
        assert_eq!(e.code, codes::INTERNAL_ERROR);
    }

    #[test]
    fn importance_spec_maps_to_core() {
        let spec = ai_protocol::ImportanceOptionsSpec::default();
        let imp = importance_from_dto(&spec);
        assert_eq!(imp, ai_core::ImportanceOptions::default(), "缺省 DTO == core 默认");
        let spec = ai_protocol::ImportanceOptionsSpec { case_unimportant: true, trailing_ws_unimportant: true, ..Default::default() };
        let imp = importance_from_dto(&spec);
        assert!(imp.case_unimportant && imp.trailing_ws_unimportant);
        assert!(!imp.leading_ws_unimportant);
    }

    #[test]
    fn manual_spec_maps_to_core() {
        let spec = ai_protocol::TextManualAlignSpec {
            align_anchors: vec![ai_protocol::TextAlignAnchor {
                left: ai_protocol::LineRange::new(0, 2),
                right: ai_protocol::LineRange::new(1, 3),
            }],
            isolate: vec![ai_protocol::SideLineRange {
                side: ai_protocol::DiffSide::Right,
                range: ai_protocol::LineRange::new(4, 6),
            }],
            ignore: vec![ai_protocol::SideLineRange {
                side: ai_protocol::DiffSide::Left,
                range: ai_protocol::LineRange::new(7, 8),
            }],
            never_align_important: true,
            skew_tolerance: 0,
            proximity_match: true,
        };
        let m = manual_from_dto(&spec);
        assert_eq!(m.align_anchors.len(), 1);
        assert_eq!(m.align_anchors[0].left.len(), 2);
        assert_eq!(m.isolate, vec![(ai_core::options::DiffSide::Right, ai_core::sections::LineRange::new(4, 6))]);
        assert_eq!(m.ignore, vec![(ai_core::options::DiffSide::Left, ai_core::sections::LineRange::new(7, 8))]);
        assert!(m.never_align_important && m.proximity_match);
        assert_eq!(m.skew_tolerance, 0, "0 = core 层取默认偏斜容差");
    }

    #[test]
    fn inline_mapping_roundtrip_shape() {
        use ai_core::{CharSpan, InlineSpan, InlineSpanKind};
        let core = ai_core::InlineLineDiff {
            left_line: 1,
            right_line: 2,
            result: ai_core::InlineLineResult {
                spans: vec![InlineSpan {
                    left: Some(CharSpan { start: 0, end: 3 }),
                    right: Some(CharSpan { start: 0, end: 4 }),
                    kind: InlineSpanKind::Replace,
                }],
                truncated: false,
            },
        };
        let dto = inline_line_diff_to_dto(&core);
        assert_eq!(dto.left_line, 1);
        assert_eq!(dto.right_line, 2);
        assert_eq!(dto.spans.len(), 1);
        assert_eq!(dto.spans[0].kind, ai_protocol::InlineSpanKind::Replace);
        assert_eq!(dto.spans[0].left, Some(ai_protocol::CharRange { start: 0, end: 3 }));
        assert!(!dto.truncated);
    }

    #[test]
    fn line_keys_entry_dual_keys() {
        let lt = ai_core::LineTable::from_text("hello\n");
        let rt = ai_core::LineTable::from_text("Hello  \n");
        let opts = ai_core::TextDiffOptions::default();
        let imp = ai_core::ImportanceOptions { trailing_ws_unimportant: true, ..Default::default() };
        let e = line_keys_entry(ai_core::options::DiffSide::Left, 0, &lt, 0, &opts, &imp);
        assert_eq!(e.side, ai_protocol::DiffSide::Left);
        assert_eq!(e.line, 0);
        assert_eq!(e.raw, "hello");
        let e = line_keys_entry(ai_core::options::DiffSide::Right, 0, &rt, 0, &opts, &imp);
        assert_eq!(e.raw, "Hello  ");
        assert_eq!(e.importance, "Hello", "尾随空白不重要键折叠（大小写仍区分）");
    }
}
