//! diff 选项（P0 固定默认值，类型先行；P1 重要性模型直接扩展此结构）。

/// 行对齐算法（docs/03 §3.1.1）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LineAlignAlgo {
    /// 标准对齐：P0 以 Myers O(ND) 一次性实现；P1 升级为分块渐进交付
    Standard,
    Myers,
    /// P1（FR-TXT-011）
    Patience,
    /// P1（FR-TXT-011）
    Histogram,
}

/// 空白三类语义（FR-TXT-010）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WhitespaceMode {
    /// 空白完全有效（P0 默认）
    Significant,
    /// 忽略空白数量：连续空白折叠为一个空格
    IgnoreAmount,
    /// 忽略所有空白
    IgnoreAll,
}

/// 行内差异粒度
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InlineLevel {
    /// 词级（空白分词，默认）
    Word,
    /// 字符级
    Char,
}

/// 行内差异选项（docs/03 §6.2：长度上限 + 超时降级）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineOptions {
    pub level: InlineLevel,
    /// 任一侧超过该字符数则不做行内 diff（`truncated = true`），默认 1000
    pub max_line_chars: usize,
    /// 单行行内 diff 超时（毫秒；0 = 不限时），默认 50
    pub timeout_ms: u64,
}

impl Default for InlineOptions {
    fn default() -> Self {
        InlineOptions {
            level: InlineLevel::Word,
            max_line_chars: DEFAULT_INLINE_MAX_LINE_CHARS,
            timeout_ms: DEFAULT_INLINE_TIMEOUT_MS,
        }
    }
}

/// 文本 diff 选项（docs/03 §3.1.1 TextDiffOptions 的 P0 子集 + 行内选项）。
///
/// P0 固定值：`Standard` + 显著空白 + 大小写有效 + 忽略行尾风格（docs/03 §6.1）。
/// `manual_anchors` / `skew_tolerance` / `proximity_match` 等 P1 字段暂不引入，
/// 避免死代码；结构演进遵循"只加字段"。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextDiffOptions {
    pub algo: LineAlignAlgo,
    pub whitespace: WhitespaceMode,
    pub ignore_case: bool,
    /// true（默认）= 行尾风格（CRLF/LF/CR）不参与比较（docs/03 §6.1）
    pub ignore_eol_style: bool,
    pub inline: InlineOptions,
}

impl Default for TextDiffOptions {
    fn default() -> Self {
        TextDiffOptions {
            algo: LineAlignAlgo::Standard,
            whitespace: WhitespaceMode::Significant,
            ignore_case: false,
            ignore_eol_style: true,
            inline: InlineOptions::default(),
        }
    }
}

/// 行内 diff 默认长度上限：单行 1000 字符（docs/03 §6.2）
pub const DEFAULT_INLINE_MAX_LINE_CHARS: usize = 1000;
/// 行内 diff 默认超时：50ms（PB-08 预算内）
pub const DEFAULT_INLINE_TIMEOUT_MS: u64 = 50;
