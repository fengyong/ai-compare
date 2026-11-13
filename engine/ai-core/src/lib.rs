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
//! 选项结构体（[`TextDiffOptions`]）本阶段固定默认值但字段齐全，
//! 且 `ignore_case` / `whitespace` / `ignore_eol_style` 的规范化键逻辑已实现，
//! 为 P1 重要性模型（raw_key / importance_key 双键）留口。

use std::borrow::Cow;

pub mod inline;
pub mod lines;
pub mod myers;
pub mod options;
pub mod sections;

pub use inline::{
    inline_diff_line, inline_diff_section, CharSpan, InlineLineDiff, InlineLineResult, InlineSpan,
    InlineSpanKind,
};
pub use lines::{LineEnding, LineEntry, LineTable};
pub use myers::{coalesce_ops, RawOp};
pub use options::{
    InlineLevel, InlineOptions, LineAlignAlgo, TextDiffOptions, WhitespaceMode,
    DEFAULT_INLINE_MAX_LINE_CHARS, DEFAULT_INLINE_TIMEOUT_MS,
};
pub use sections::{
    build_sections, DiffError, DiffOutput, DiffSummary, Importance, Section, SectionEvent,
    SectionId, SectionKind,
};

/// 把两侧文本做行级 diff（一次性；流式入口见 [`diff_stream`]）。
///
/// diff 在"规范化比较键"上运算、在原文上渲染（docs/03 §3.1.1）：
/// 默认选项下键即原文（零拷贝借用），开启忽略空白/大小写/行尾后才产生 owned 键。
pub fn diff_text(
    left: &LineTable,
    right: &LineTable,
    opts: &TextDiffOptions,
) -> Result<DiffOutput, DiffError> {
    if opts.algo == LineAlignAlgo::Patience || opts.algo == LineAlignAlgo::Histogram {
        return Err(DiffError::UnsupportedAlgorithm(opts.algo));
    }
    // P0：Standard 与 Myers 均走 O(ND) 一次性实现（Standard 的分块渐进在 P1 升级）。
    let a: Vec<Cow<'_, str>> = (0..left.len()).map(|i| left.line(i).compare_key(opts)).collect();
    let b: Vec<Cow<'_, str>> =
        (0..right.len()).map(|i| right.line(i).compare_key(opts)).collect();
    let ops = myers::diff_seq(&a, &b);
    let out = sections::build_sections(&ops);
    Ok(out)
}

/// 流式输出：section 按序回调 `Append`，结束回调 `Done`（docs/03 §4.1 SectionEvent）。
/// Standard 渐进交付（分块）在 P1 引入；P0 事件序列等价于一次性结果。
pub fn diff_stream(
    left: &LineTable,
    right: &LineTable,
    opts: &TextDiffOptions,
    sink: &mut dyn FnMut(SectionEvent),
) -> Result<DiffSummary, DiffError> {
    let out = diff_text(left, right, opts)?;
    for s in &out.sections {
        sink(SectionEvent::Append(*s));
    }
    sink(SectionEvent::Done(out.summary));
    Ok(out.summary)
}

#[cfg(test)]
mod tests;
