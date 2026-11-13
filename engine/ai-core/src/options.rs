//! diff 选项。
//!
//! - P0 骨架选项 [`TextDiffOptions`] 字段集冻结（引擎 mapping 层以全字段字面量构造，
//!   只能"只加不改"——新增配置一律放到独立结构体，见 [`ImportanceOptions`]、
//!   [`ManualAlignOptions`] 与 [`DiffConfig`]）。
//! - P1 新增（docs/04 P1 交付物 ②③④⑤）：
//!   - [`ImportanceOptions`]：重要性模型（FR-TXT-010），空白三类/大小写/行尾/编码
//!     可配置为「不重要」；
//!   - [`ManualAlignOptions`]：手动对齐锚点、隔离（从不对齐）、偏斜容差、接近度
//!     匹配、手动忽略（FR-TXT-011）；
//!   - [`SemanticCleanup`]：行内 diff 的 DMP 语义清理策略（可选，默认关闭保持 P0 行为）。

/// 行对齐算法（docs/03 §3.1.1、FR-TXT-011）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LineAlignAlgo {
    /// 标准对齐：分块渐进（先粗后细）——公共前缀/后缀裁剪 + 唯一行锚点分块，
    /// 各块独立 diff 并可渐进交付（[`crate::progressive::ProgressiveDiffer`]）
    Standard,
    /// Myers O(ND)：一次性比较（大插入、大量重复文本时匹配更好）
    Myers,
    /// Patience Diff（Bram Cohen）：仅以两侧均唯一出现的行做锚点递归
    Patience,
    /// Histogram（git 同源，imara-diff 接入）：patience 的低频锚点泛化
    Histogram,
}

/// 空白三类语义（FR-TXT-010）——作用于**骨架规范化键**（P0 语义：
/// 开启后空白差异在行对齐层面直接消失）。
///
/// P1 重要性模型（空白不重要但骨架不变）见 [`ImportanceOptions`]。
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

/// 行内 diff 语义清理策略（DMP diff-match-patch 的简化对应物）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SemanticCleanup {
    /// 不清理（P0 默认行为，区间最小化）
    #[default]
    None,
    /// DMP 语义清理（简化版）：合并被 ≤ [`DMP_SEMANTIC_EDIT_COST`] 字符的
    /// 等值段分隔的相邻编辑区间，使编辑块符合"语义整体"的直觉边界
    Dmp,
}

/// 行内差异选项（docs/03 §6.2：长度上限 + 超时降级；P1 增加语义清理策略）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineOptions {
    pub level: InlineLevel,
    /// 任一侧超过该字符数则不做行内 diff（`truncated = true`），默认 1000。
    /// 可配置（FR-BIG / docs/03 §6.2 行内策略）。
    pub max_line_chars: usize,
    /// 单行行内 diff 超时（毫秒；0 = 不限时），默认 50。
    /// P1 起为**真超时**：Myers 中点搜索按 deadline 抢占，超时降级为整行差异。
    pub timeout_ms: u64,
    /// 语义清理策略（默认 [`SemanticCleanup::None`]，P0 行为不变）
    pub semantic_cleanup: SemanticCleanup,
}

impl Default for InlineOptions {
    fn default() -> Self {
        InlineOptions {
            level: InlineLevel::Word,
            max_line_chars: DEFAULT_INLINE_MAX_LINE_CHARS,
            timeout_ms: DEFAULT_INLINE_TIMEOUT_MS,
            semantic_cleanup: SemanticCleanup::None,
        }
    }
}

/// 重要性模型选项（FR-TXT-010，docs/04 P1 交付物 ②）。
///
/// 语义：勾选「不重要」的方面在**重要性规范化键**（importance key）中被折叠；
/// diff 骨架始终在 raw 键上计算，因此切换这些开关**不改变行对齐骨架**，
/// 只触发 section 重要性重判（[`crate::SectionEvent::ImportanceUpdate`]）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ImportanceOptions {
    /// 行首空白不重要
    pub leading_ws_unimportant: bool,
    /// 嵌入空白不重要（连续空白折叠为一个空格）
    pub embedded_ws_unimportant: bool,
    /// 行尾空白不重要
    pub trailing_ws_unimportant: bool,
    /// 字符大小写不重要（"其他所有内容"区分大小写开关的反面）
    pub case_unimportant: bool,
    /// 行尾风格（CRLF/LF/CR）不重要。默认 true（docs/03 §6.1：比较默认忽略行尾）；
    /// 勾选「比较行尾」（且骨架层 `ignore_eol_style = false`）时应设为 false。
    pub eol_style_unimportant: bool,
    /// 纯编码差异不重要。编码差异在解码层已消除（两侧解码文本相同即键相同），
    /// 此开关为规则系统占位：true 时引擎上层无需对"仅编码不同"的文件产生 diff。
    pub encoding_unimportant: bool,
}

impl Default for ImportanceOptions {
    fn default() -> Self {
        ImportanceOptions {
            leading_ws_unimportant: false,
            embedded_ws_unimportant: false,
            trailing_ws_unimportant: false,
            case_unimportant: false,
            eol_style_unimportant: true,
            encoding_unimportant: true,
        }
    }
}

/// 手动对齐锚点：强制左右行区间并排（FR-TXT-011 手动对齐）。
///
/// 两区间行数须相等；区间按序、不重叠、单调递增。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AlignAnchor {
    pub left: crate::sections::LineRange,
    pub right: crate::sections::LineRange,
}

/// 隔离区间（FR-TXT-011 编辑 > 隔离）：指定行区间「从不对齐」——
/// 这些行不与任何对侧行配对，独立成 Delete/Insert 块。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiffSide {
    Left,
    Right,
}

/// 手动对齐/隔离/忽略选项（FR-TXT-011，docs/04 P1 交付物 ③）
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ManualAlignOptions {
    /// 手动对齐锚点（强制并排）
    pub align_anchors: Vec<AlignAnchor>,
    /// 隔离：这些行区间从不对齐
    pub isolate: Vec<(DiffSide, crate::sections::LineRange)>,
    /// 从不对齐差异：重要差异行显示为「添加 + 删除」的文本块而非「更改」
    pub never_align_important: bool,
    /// 偏斜容差：接近度匹配在对侧搜索匹配行的最大行数；
    /// 增大改善大间隙对齐但更慢（0 = 使用 [`DEFAULT_SKEW_TOLERANCE`]）
    pub skew_tolerance: u32,
    /// 使用接近度匹配：按相似度尝试对齐剩余不匹配行
    /// （实现：对被 ≤ 偏斜容差行等值段分隔的 删除+插入 块做窗口重 diff）
    pub proximity_match: bool,
    /// 手动忽略：与这些区间重叠的差异 section 标记为 [`crate::Importance::Ignored`]
    pub ignore: Vec<(DiffSide, crate::sections::LineRange)>,
}

/// 偏斜容差默认值（0 表示使用此值）
pub const DEFAULT_SKEW_TOLERANCE: u32 = 2500;

impl Default for ManualAlignOptions {
    fn default() -> Self {
        ManualAlignOptions {
            align_anchors: Vec::new(),
            isolate: Vec::new(),
            never_align_important: false,
            skew_tolerance: DEFAULT_SKEW_TOLERANCE,
            proximity_match: false,
            ignore: Vec::new(),
        }
    }
}

impl ManualAlignOptions {
    /// 有效偏斜容差（0 视为默认值）
    pub fn effective_skew(&self) -> u32 {
        if self.skew_tolerance == 0 {
            DEFAULT_SKEW_TOLERANCE
        } else {
            self.skew_tolerance
        }
    }

    /// 校验锚点：等长、单调、不重叠、在行数范围内。返回 Err(说明) 表示无效。
    pub fn validate(&self, left_len: u32, right_len: u32) -> Result<(), crate::sections::DiffError> {
        let mut prev_l = 0u32;
        let mut prev_r = 0u32;
        for a in &self.align_anchors {
            if a.left.len() != a.right.len() {
                return Err(crate::sections::DiffError::InvalidManualAnchor(format!(
                    "锚点区间行数不等：left {} != right {}",
                    a.left.len(),
                    a.right.len()
                )));
            }
            if a.left.start < prev_l || a.right.start < prev_r {
                return Err(crate::sections::DiffError::InvalidManualAnchor(
                    "锚点必须单调递增且不重叠".into(),
                ));
            }
            if a.left.end > left_len || a.right.end > right_len {
                return Err(crate::sections::DiffError::InvalidManualAnchor(format!(
                    "锚点越界：left {a:?} right {a:?}（行数 {left_len}/{right_len}）"
                )));
            }
            prev_l = a.left.end;
            prev_r = a.right.end;
        }
        Ok(())
    }
}

/// P1 完整 diff 配置：P0 骨架选项 + 重要性模型 + 手动对齐。
///
/// 兼容性说明：`base` 保持 P0 字段集（引擎 mapping 层逐字段构造）；
/// 默认值下（无重要性折叠、无手动锚点）行为与 P0 `diff_text` 完全一致。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DiffConfig {
    /// P0 骨架选项（算法、骨架规范化、行内选项）
    pub base: TextDiffOptions,
    /// 重要性模型（FR-TXT-010）
    pub importance: ImportanceOptions,
    /// 手动对齐/隔离（FR-TXT-011）
    pub manual: ManualAlignOptions,
}

/// 文本 diff 选项（docs/03 §3.1.1 TextDiffOptions 的 P0 子集 + 行内选项）。
///
/// **字段集冻结**：引擎 mapping 层以全字段字面量构造本结构，新增配置进
/// [`DiffConfig`] / [`ImportanceOptions`] / [`ManualAlignOptions`]。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextDiffOptions {
    pub algo: LineAlignAlgo,
    pub whitespace: WhitespaceMode,
    pub ignore_case: bool,
    /// true（默认）= 行尾风格（CRLF/LF/CR）不参与骨架比较（docs/03 §6.1）
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

/// DMP 语义清理：被不超过该字符数的等值段分隔的编辑区间将被合并
/// （diff-match-patch `diff_cleanupSemantic` 的 `EditCost` 简化对应）
pub const DMP_SEMANTIC_EDIT_COST: usize = 4;

/// 行内 diff 默认长度上限：单行 1000 字符（docs/03 §6.2）
pub const DEFAULT_INLINE_MAX_LINE_CHARS: usize = 1000;
/// 行内 diff 默认超时：50ms（PB-08 预算内）
pub const DEFAULT_INLINE_TIMEOUT_MS: u64 = 50;
