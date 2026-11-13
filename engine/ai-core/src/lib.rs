//! # ai-core（M01）：diff 核心算法
//!
//! 纯函数、无 IO 感知（docs/03 §2.2 L2 规则：只面对"行数组/字节流/元数据"）。
//!
//! P0 范围（docs/04 P0 交付物 ③）：
//! - 行分割（CRLF/LF/CR/无尾换行/空文件全部保留信息）；
//! - 行级 diff：**Standard 对齐** —— P0 以 Myers O(ND)（线性空间分治）实现，
//!   P1 升级为分块渐进 + Patience/Histogram；
//! - 行内（词级/字符级）差异，含长度上限与超时降级（docs/03 §6.2）；
//! - 输出 section 化流式模型（docs/03 §4.1 TextDiffResult：
//!   equal/insert/delete/change section 序列 + `SectionEvent` 流）。
//!
//! P1 范围（docs/04 P1 交付物 ①~⑥）：
//! - 算法矩阵：Myers（原创）/ Patience（原创）/ Histogram（imara-diff）/
//!   Standard（唯一行锚点分块渐进，先粗后细），统一 [`LineAlignAlgo`] 切换；
//! - 重要性规范化双键（[`keys`]：raw/importance；FR-TXT-010）；
//! - 手动对齐/隔离/从不对齐/偏斜容差/接近度匹配（[`manual`]；FR-TXT-011）；
//! - 编辑增量重 diff（[`incremental::DiffEngine`]，Replace 事件范围有界）；
//! - 行内 diff 增强（deadline 抢占超时 + DMP 语义清理可选）；
//! - 对拍测试矩阵（difflib/git diff golden，`tests/` 下）+ PB-05 算法矩阵基准。
//!
//! 兼容性：P0 入口 [`diff_text`] / [`diff_stream`] 签名不变；
//! `TextDiffOptions` P0 字段集冻结，P1 新配置进 [`DiffConfig`]；P10 新增
//! `rules` 字段（[`rules::RulesOptions`]，缺省空 = 行为不变）。默认
//! [`DiffConfig`] 下行为与 P0 完全一致。

use std::borrow::Cow;

pub mod formats;
pub mod histogram;
pub mod hex;
pub mod incremental;
pub mod inline;
pub mod keys;
pub mod lines;
pub mod manual;
pub mod merge;
pub mod myers;
pub mod options;
pub mod patch;
pub mod patience;
pub mod pipeline;
pub mod progressive;
pub mod rules;
pub mod sections;
pub mod table;

pub use formats::{
    builtin_registry, line_classify, mask_matches, sniff_bytes, sniff_shebang, unimportant_mask,
    FileClass, FileFormat, FormatPack, FormatPackError, FormatRegistry, FormatViewKind,
    GrammarElement, LineClass, LineWeight, SyntaxItem, UNIMPORTANT_MARK,
};
pub use rules::{
    CompareRules, EffectiveRules, FormatOverride, FormatsDocument, RuleFormatBinding, RuleLayer,
    RuleSet, RuleTarget, RulesDocument, RulesOptions,
};

pub use inline::{
    inline_diff_line, inline_diff_section, CharSpan, InlineLineDiff, InlineLineResult, InlineSpan,
    InlineSpanKind,
};
pub use incremental::{DiffEngine, TextEdit};
pub use keys::{
    aggregate_unimportant, apply_classification, classify_sections, importance_key, raw_key,
    KeyedDiffOutput, LineKeyPair, LineKeys, SectionKeyInfo,
};
pub use lines::{LineEnding, LineEntry, LineTable};
pub use merge::{
    auto_merge_text, merge3, synthesize, ChunkOrigin, ConflictDefault, ConflictDetail, ConflictKind,
    ConflictRow, FavorSide, MarkerOptions, MarkerStyle, MergeChunk, MergeDecision, MergeModel,
    MergeOptions, MergeOutput, SynthesizeOptions, UnresolvedPolicy,
};
pub use myers::{coalesce_ops, diff_seq_deadline, RawOp};
pub use options::{
    DiffConfig, DiffSide, ImportanceOptions, InlineLevel, InlineOptions, LineAlignAlgo,
    ManualAlignOptions, SemanticCleanup, TextDiffOptions, WhitespaceMode,
    AlignAnchor, DEFAULT_INLINE_MAX_LINE_CHARS, DEFAULT_INLINE_TIMEOUT_MS, DEFAULT_SKEW_TOLERANCE,
    DMP_SEMANTIC_EDIT_COST,
};
pub use progressive::ProgressiveDiffer;
pub use sections::{
    build_sections, DiffError, DiffOutput, DiffSummary, Importance, Section, SectionEvent,
    SectionId, SectionKind,
};

/// 把两侧文本做行级 diff（一次性；P0 兼容入口）。
///
/// diff 在"规范化比较键"上运算、在原文上渲染（docs/03 §3.1.1）：
/// 默认选项下键即原文（零拷贝借用），开启忽略空白/大小写/行尾后才产生 owned 键。
///
/// P1 起支持全部四种算法；P0 语义（Standard == 一次性 Myers）由
/// [`diff_config`] 的分块管线在无锚点退化的简单输入上保持一致，
/// 复杂输入的对齐形态以 Standard 自己的分块语义为准（FR-TXT-011）。
pub fn diff_text(
    left: &LineTable,
    right: &LineTable,
    opts: &TextDiffOptions,
) -> Result<DiffOutput, DiffError> {
    let cfg = DiffConfig { base: opts.clone(), ..DiffConfig::default() };
    let keyed = diff_config(left, right, &cfg)?;
    Ok(keyed.output)
}

/// P1 完整入口：骨架选项 + 重要性模型 + 手动对齐（docs/04 P1）。
pub fn diff_config(
    left: &LineTable,
    right: &LineTable,
    cfg: &DiffConfig,
) -> Result<KeyedDiffOutput, DiffError> {
    pipeline::run_full(left, right, cfg)
}

/// 流式输出（P0 兼容入口）：section 按序回调，结束回调 `Done`。
///
/// Standard 渐进交付（FR-TXT-011）：先粗粒度 `Append`（分块占位），
/// 再逐块 `Replace` 精化（先粗后细，FR-BIG-001）；其余算法一次性交付。
pub fn diff_stream(
    left: &LineTable,
    right: &LineTable,
    opts: &TextDiffOptions,
    sink: &mut dyn FnMut(SectionEvent),
) -> Result<DiffSummary, DiffError> {
    let cfg = DiffConfig { base: opts.clone(), ..DiffConfig::default() };
    let keyed = diff_config(left, right, &cfg)?;
    for s in &keyed.output.sections {
        sink(SectionEvent::Append(*s));
    }
    sink(SectionEvent::Done(keyed.output.summary));
    Ok(keyed.output.summary)
}

/// 返回值规范化借用键（供外部测试兼容；见 [`LineTable::compare_key`]）。
pub fn compare_key<'a>(table: &'a LineTable, i: usize, opts: &TextDiffOptions) -> Cow<'a, str> {
    table.compare_key(i, opts)
}

#[cfg(test)]
mod tests;
