//! 规则体系（P10 / FR-TXT-010 / FR-SES-006 引擎侧；docs/03 §4.4 重要性模型）。
//!
//! 三层协作（SRS 2.4）：
//! - **格式层**（[`crate::formats`]）定义语法元素的具体匹配（细节层）；
//! - **规则层**（本模块）声明"勾选哪些元素不重要"（抽象层，引用元素名）与
//!   比较开关（空白三类/大小写/行尾/编码）；
//! - **作用域层**：规则按 Global → TypeDefault → Session 继承（FR-SES-006，
//!   复用会话作用域语义：读时逐层合并、深层覆盖浅层）。
//!
//! 与 P1 重要性键的接线：
//! - `CompareRules` → [`ImportanceOptions`]（空白三类/大小写/行尾/编码折叠）；
//! - `unimportant_elements` + 格式语法 → [`RulesOptions`]（语法折叠：
//!   注释行/字符串内容在重要性键中折叠为占位符；**骨架键不受影响**，
//!   行对齐不变、仅触发重要性重判，docs/03 §4.1）。
//!
//! `TextDiffOptions` 扩展：新增 [`RulesOptions`] 字段 `rules`（缺省 = 空，
//! 行为与 P0/P1 完全一致——向后兼容默认值；协议载荷为 ai-protocol 独立
//! DTO，本结构不直接上协议）。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::formats::{unimportant_mask, GrammarElement, FileFormat};
use crate::options::ImportanceOptions;

// ---------------------------------------------------------------------------
// 比较规则（会话设置 > 重要性 的开关面；FR-TXT-010）
// ---------------------------------------------------------------------------

/// 比较开关（与 [`ImportanceOptions`] 同形；Default 语义一致：
/// 行尾风格与纯编码差异不重要，docs/03 §6.1）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CompareRules {
    /// 行首空白不重要
    #[serde(default)]
    pub leading_ws_unimportant: bool,
    /// 嵌入空白不重要（连续空白折叠为一个空格）
    #[serde(default)]
    pub embedded_ws_unimportant: bool,
    /// 行尾空白不重要
    #[serde(default)]
    pub trailing_ws_unimportant: bool,
    /// 字符大小写不重要（"其他所有内容"区分大小写开关的反面）
    #[serde(default)]
    pub case_unimportant: bool,
    /// 行尾风格（CRLF/LF/CR）不重要。缺省 true（比较默认忽略行尾）
    #[serde(default = "default_true")]
    pub eol_style_unimportant: bool,
    /// 纯编码差异不重要。缺省 true
    #[serde(default = "default_true")]
    pub encoding_unimportant: bool,
}

fn default_true() -> bool {
    true
}

impl Default for CompareRules {
    fn default() -> Self {
        CompareRules {
            leading_ws_unimportant: false,
            embedded_ws_unimportant: false,
            trailing_ws_unimportant: false,
            case_unimportant: false,
            eol_style_unimportant: true,
            encoding_unimportant: true,
        }
    }
}

impl From<CompareRules> for ImportanceOptions {
    fn from(c: CompareRules) -> Self {
        ImportanceOptions {
            leading_ws_unimportant: c.leading_ws_unimportant,
            embedded_ws_unimportant: c.embedded_ws_unimportant,
            trailing_ws_unimportant: c.trailing_ws_unimportant,
            case_unimportant: c.case_unimportant,
            eol_style_unimportant: c.eol_style_unimportant,
            encoding_unimportant: c.encoding_unimportant,
        }
    }
}

/// 一层规则（可部分指定；None = 该项不覆盖，回落继承）
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RuleLayer {
    /// 比较开关覆盖（None 字段回落继承层）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compare: Option<CompareRules>,
    /// 不重要语法元素名集（None = 不覆盖；空集 = 显式清空该层）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unimportant_elements: Option<Vec<String>>,
}

/// 格式绑定：该格式的会话元素勾选（抽象层元素名集按格式分别记忆，FR-TXT-010
/// "重要性选项卡列出当前文件格式的语法元素复选框"）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleFormatBinding {
    pub format_id: String,
    #[serde(default)]
    pub unimportant_elements: Vec<String>,
}

// ---------------------------------------------------------------------------
// 作用域继承：Global → TypeDefault → Session（FR-SES-006）
// ---------------------------------------------------------------------------

/// 规则集（引擎进程内权威态；持久化形态见 [`RulesDocument`]）
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RuleSet {
    /// 全局层（所有会话类型）
    #[serde(default)]
    pub global: RuleLayer,
    /// 会话类型默认层（键 = SessionKind 字符串，如 "text-compare"）
    #[serde(default)]
    pub type_defaults: BTreeMap<String, RuleLayer>,
    /// 会话实例层（键 = 引擎 sessionId）
    #[serde(default)]
    pub sessions: BTreeMap<String, RuleLayer>,
    /// 格式绑定（format_id → 元素勾选；解析时最后应用）
    #[serde(default)]
    pub format_bindings: Vec<RuleFormatBinding>,
}

/// 解析后的生效规则
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EffectiveRules {
    pub compare: CompareRules,
    /// 不重要语法元素名（引用格式层元素名）
    pub unimportant_elements: Vec<String>,
}

impl EffectiveRules {
    pub fn to_importance_options(&self) -> ImportanceOptions {
        self.compare.into()
    }
}

impl RuleSet {
    /// 作用域解析：Global → TypeDefault(kind) → Session(session_id)，
    /// 深层覆盖浅层（字段级覆盖：compare 整体覆盖、元素集整体覆盖）；
    /// 末尾应用格式绑定（format_id 命中时其元素集覆盖会话层元素集——
    /// 元素勾选按格式记忆，BC 语义）。
    pub fn resolve(
        &self,
        kind: Option<&str>,
        session_id: Option<&str>,
        format_id: Option<&str>,
    ) -> EffectiveRules {
        let mut compare = CompareRules::default();
        let mut elements: Option<Vec<String>> = None;
        let mut apply = |layer: &RuleLayer| {
            if let Some(c) = &layer.compare {
                compare = *c;
            }
            if layer.unimportant_elements.is_some() {
                elements = layer.unimportant_elements.clone();
            }
        };
        apply(&self.global);
        if let Some(k) = kind {
            if let Some(l) = self.type_defaults.get(k) {
                apply(l);
            }
        }
        if let Some(s) = session_id {
            if let Some(l) = self.sessions.get(s) {
                apply(l);
            }
        }
        if let Some(fid) = format_id {
            if let Some(b) = self.format_bindings.iter().find(|b| b.format_id == fid) {
                elements = Some(b.unimportant_elements.clone());
            }
        }
        EffectiveRules {
            compare,
            unimportant_elements: elements.unwrap_or_default(),
        }
    }

    /// 写一层（rules/set；scope 由调用方定位）
    pub fn set_layer(&mut self, target: RuleTarget, layer: RuleLayer) {
        match target {
            RuleTarget::Global => self.global = layer,
            RuleTarget::TypeDefault(kind) => {
                self.type_defaults.insert(kind, layer);
            }
            RuleTarget::Session(id) => {
                self.sessions.insert(id, layer);
            }
        }
    }
}

/// rules/set 的写入目标
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleTarget {
    Global,
    TypeDefault(String),
    Session(String),
}

// ---------------------------------------------------------------------------
// TextDiffOptions 扩展载体：语法折叠选项（向后兼容默认值）
// ---------------------------------------------------------------------------

/// 语法折叠选项（挂在 `TextDiffOptions.rules`；Default 空 = 与既有行为一致）。
///
/// 作用点：`keys::importance_key` 先按本选项做 [`unimportant_mask`] 折叠，
/// 再走空白/大小写折叠；骨架键（raw_key）不受影响（行对齐不变）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RulesOptions {
    /// 不重要语法元素名（引用格式层）
    pub unimportant_elements: Vec<String>,
    /// 生效格式携带的语法元素（通常为该格式的 grammar 子集/全集）
    pub grammar: Vec<GrammarElement>,
}

impl RulesOptions {
    /// 是否启用语法折叠（两要素齐备才生效）
    pub fn has_folding(&self) -> bool {
        !self.unimportant_elements.is_empty() && !self.grammar.is_empty()
    }

    /// 从生效规则 + 命中格式构建（格式未命中时 grammar 为空 → 不折叠）
    pub fn resolve(effective: &EffectiveRules, format: Option<&FileFormat>) -> Self {
        let grammar = match format {
            Some(f) if !effective.unimportant_elements.is_empty() => f.grammar.clone(),
            _ => Vec::new(),
        };
        RulesOptions { unimportant_elements: effective.unimportant_elements.clone(), grammar }
    }

    /// 行重要性键预处理（keys 层调用；无折叠时原样返回以保持零分配路径）
    pub fn fold_line(&self, text: &str, line_index: Option<usize>) -> String {
        if !self.has_folding() {
            return text.to_string();
        }
        unimportant_mask(text, &self.grammar, &self.unimportant_elements, line_index)
    }
}

// ---------------------------------------------------------------------------
// 持久化模型（与 UI 会话持久化同层的 formats.json / rules.json 结构）
// ---------------------------------------------------------------------------

/// rules.json 文档（版本化；UI 与引擎共享同一 JSON 形态）
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RulesDocument {
    /// 文档版本（当前 1）
    pub version: u32,
    #[serde(default)]
    pub global: RuleLayer,
    #[serde(default)]
    pub type_defaults: BTreeMap<String, RuleLayer>,
    #[serde(default)]
    pub sessions: BTreeMap<String, RuleLayer>,
    #[serde(default)]
    pub format_bindings: Vec<RuleFormatBinding>,
}

pub const RULES_DOCUMENT_VERSION: u32 = 1;

impl RuleSet {
    pub fn to_document(&self) -> RulesDocument {
        RulesDocument {
            version: RULES_DOCUMENT_VERSION,
            global: self.global.clone(),
            type_defaults: self.type_defaults.clone(),
            sessions: self.sessions.clone(),
            format_bindings: self.format_bindings.clone(),
        }
    }

    pub fn from_document(doc: &RulesDocument) -> Self {
        RuleSet {
            global: doc.global.clone(),
            type_defaults: doc.type_defaults.clone(),
            sessions: doc.sessions.clone(),
            format_bindings: doc.format_bindings.clone(),
        }
    }
}

/// formats.json 文档：自定义格式 + 内置覆盖（enabled/掩码/顺序）
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FormatsDocument {
    /// 文档版本（当前 1）
    pub version: u32,
    /// 用户自定义格式（追加在内置表之后；id 不得与内置冲突，冲突即覆盖）
    #[serde(default)]
    pub custom: Vec<FileFormat>,
    /// 内置格式覆盖（按 id 定位）
    #[serde(default)]
    pub builtin_overrides: Vec<FormatOverride>,
}

pub const FORMATS_DOCUMENT_VERSION: u32 = 1;

/// 内置格式的覆盖项（部分字段可选）
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FormatOverride {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub masks: Option<Vec<String>>,
    /// 上移/下移后的绝对顺序位（0 起；None = 不动）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<u32>,
}

impl FormatsDocument {
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }

    pub fn from_json(text: &str) -> Result<Self, String> {
        serde_json::from_str(text).map_err(|e| e.to_string())
    }
}

impl RulesDocument {
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }

    pub fn from_json(text: &str) -> Result<Self, String> {
        serde_json::from_str(text).map_err(|e| e.to_string())
    }
}

// ---------------------------------------------------------------------------
// 测试：规则应用行为（≥10：注释行不算差异/字符串/作用域继承/持久化 roundtrip）
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::builtin_registry;
    use crate::options::{TextDiffOptions, WhitespaceMode};
    use crate::sections::Importance;
    use crate::{diff_config, LineTable};
    use crate::options::DiffConfig;
    use crate::options::ManualAlignOptions;

    fn c_format() -> &'static FileFormat {
        builtin_registry().detect("main.c", b"").unwrap()
    }

    fn rules_options(elements: &[&str], format: Option<&FileFormat>) -> RulesOptions {
        let effective = EffectiveRules {
            compare: CompareRules::default(),
            unimportant_elements: elements.iter().map(|s| s.to_string()).collect(),
        };
        RulesOptions::resolve(&effective, format)
    }

    fn config_with(rules: RulesOptions, imp: ImportanceOptions) -> DiffConfig {
        DiffConfig {
            base: TextDiffOptions { rules, ..TextDiffOptions::default() },
            importance: imp,
            manual: ManualAlignOptions::default(),
        }
    }

    // ---- 1. 注释行不算差异（重要性重判，骨架不变） ----

    #[test]
    fn r01_comment_only_change_is_unimportant() {
        let left = LineTable::from_text("int a;\n// old comment\nint b;\n");
        let right = LineTable::from_text("int a;\n// new comment\nint b;\n");
        let rules = rules_options(&["注释"], Some(c_format()));
        let keyed = diff_config(&left, &right, &config_with(rules, ImportanceOptions::default())).unwrap();
        assert_eq!(keyed.output.sections.len(), 3, "骨架：Equal + Change + Equal");
        assert_eq!(keyed.output.sections[1].important, Importance::Unimportant, "注释行差异不重要");
        assert_eq!(keyed.unimportant_sections, 1);
    }

    #[test]
    fn r02_comment_rule_off_keeps_important() {
        let left = LineTable::from_text("// old\n");
        let right = LineTable::from_text("// new\n");
        // 无语法折叠（rules 空）→ 注释差异重要
        let keyed = diff_config(&left, &right, &config_with(RulesOptions::default(), ImportanceOptions::default())).unwrap();
        assert_eq!(keyed.output.sections[0].important, Importance::Important);
    }

    #[test]
    fn r03_orphan_comment_is_still_important() {
        // 孤儿行（仅一侧存在的注释行）始终重要（FR-TXT-010）
        let left = LineTable::from_text("a\n// gone\nb\n");
        let right = LineTable::from_text("a\nb\n");
        let rules = rules_options(&["注释"], Some(c_format()));
        let keyed = diff_config(&left, &right, &config_with(rules, ImportanceOptions::default())).unwrap();
        let del = keyed
            .output
            .sections
            .iter()
            .find(|s| s.kind == crate::sections::SectionKind::Delete)
            .expect("孤儿注释行应成 Delete 段");
        assert_eq!(del.important, Importance::Important);
    }

    #[test]
    fn r04_skeleton_not_affected_by_grammar_folding() {
        // 语法折叠只影响重要性键；raw 键（骨架）不变
        let left = LineTable::from_text("// alpha\n");
        let right = LineTable::from_text("// beta\n");
        let rules = rules_options(&["注释"], Some(c_format()));
        let cfg = config_with(rules, ImportanceOptions::default());
        let keyed = diff_config(&left, &right, &cfg).unwrap();
        assert_eq!(keyed.output.sections.len(), 1, "骨架仍为单 Change（对齐不变）");
        let sec = &keyed.output.sections[0];
        assert_eq!(sec.kind, crate::sections::SectionKind::Change);
        assert!(sec.important == Importance::Unimportant);
    }

    // ---- 2. 字符串内容不算差异 ----

    #[test]
    fn r05_string_content_change_unimportant() {
        let left = LineTable::from_text("x = \"alpha\";\n");
        let right = LineTable::from_text("x = \"beta\";\n");
        let rules = rules_options(&["字符串"], Some(c_format()));
        let keyed = diff_config(&left, &right, &config_with(rules, ImportanceOptions::default())).unwrap();
        assert_eq!(keyed.output.sections[0].important, Importance::Unimportant, "字符串内容差异不重要");
    }

    #[test]
    fn r06_string_change_with_code_change_stays_important() {
        let left = LineTable::from_text("x = \"alpha\";\n");
        let right = LineTable::from_text("y = \"beta\";\n");
        let rules = rules_options(&["字符串"], Some(c_format()));
        let keyed = diff_config(&left, &right, &config_with(rules, ImportanceOptions::default())).unwrap();
        assert_eq!(keyed.output.sections[0].important, Importance::Important, "代码标识符变化仍重要");
    }

    // ---- 3. 与空白/大小写开关的叠加 ----

    #[test]
    fn r07_case_and_comment_flags_compose() {
        let left = LineTable::from_text("// note\nValue = 1\n");
        let right = LineTable::from_text("// NOTE\nvalue = 1\n");
        let rules = rules_options(&["注释"], Some(c_format()));
        let imp = ImportanceOptions { case_unimportant: true, ..ImportanceOptions::default() };
        let keyed = diff_config(&left, &right, &config_with(rules, imp)).unwrap();
        assert!(keyed.unimportant_sections >= 1, "注释差异与大小写差异均不重要（可能合并为单 section）");
        assert!(
            keyed.output.sections.iter().all(|s| s.kind != crate::sections::SectionKind::Change
                || s.important == Importance::Unimportant),
            "全部 Change 均不重要"
        );
    }

    #[test]
    fn r08_whitespace_flag_unaffected_by_grammar() {
        let left = LineTable::from_text("int   a;\n");
        let right = LineTable::from_text("int a;\n");
        let rules = rules_options(&["注释"], Some(c_format()));
        let mut imp = ImportanceOptions::default();
        imp.embedded_ws_unimportant = true;
        let keyed = diff_config(&left, &right, &config_with(rules.clone(), imp)).unwrap();
        assert_eq!(keyed.output.sections[0].important, Importance::Unimportant);
        // 空白重要时仍重要（语法折叠不吞代码空白）
        let keyed2 = diff_config(&left, &right, &config_with(rules, ImportanceOptions::default())).unwrap();
        assert_eq!(keyed2.output.sections[0].important, Importance::Important);
    }

    // ---- 4. 作用域继承 Global → TypeDefault → Session ----

    #[test]
    fn r09_scope_inheritance_layering() {
        let mut set = RuleSet::default();
        set.global = RuleLayer {
            compare: Some(CompareRules { case_unimportant: true, ..CompareRules::default() }),
            unimportant_elements: Some(vec!["注释".into()]),
        };
        set.type_defaults.insert(
            "text-compare".into(),
            RuleLayer {
                compare: Some(CompareRules { trailing_ws_unimportant: true, ..CompareRules::default() }),
                unimportant_elements: None,
            },
        );
        set.sessions.insert(
            "s1".into(),
            RuleLayer {
                compare: Some(CompareRules { case_unimportant: false, ..CompareRules::default() }),
                unimportant_elements: Some(vec!["字符串".into()]),
            },
        );

        // 仅全局
        let e = set.resolve(None, None, None);
        assert!(e.compare.case_unimportant);
        assert_eq!(e.unimportant_elements, vec!["注释".to_string()]);

        // 全局 → 类型默认（元素集继承全局；compare 覆盖为类型层）
        let e = set.resolve(Some("text-compare"), None, None);
        assert!(e.compare.trailing_ws_unimportant, "类型层 compare 整体覆盖");
        assert!(!e.compare.case_unimportant, "类型层未开启的项回落缺省（整体覆盖语义）");
        assert_eq!(e.unimportant_elements, vec!["注释".to_string()], "元素集 None 回落继承");

        // 全局 → 类型 → 会话
        let e = set.resolve(Some("text-compare"), Some("s1"), None);
        assert!(!e.compare.case_unimportant, "会话层显式关闭");
        assert_eq!(e.unimportant_elements, vec!["字符串".to_string()], "会话层元素集覆盖");
    }

    #[test]
    fn r10_format_binding_overrides_elements() {
        let mut set = RuleSet::default();
        set.global.unimportant_elements = Some(vec!["注释".into(), "字符串".into()]);
        set.format_bindings.push(RuleFormatBinding {
            format_id: "builtin-cpp".into(),
            unimportant_elements: vec!["注释".into()],
        });
        let e = set.resolve(None, None, Some("builtin-cpp"));
        assert_eq!(e.unimportant_elements, vec!["注释".to_string()], "命中格式绑定覆盖");
        let e2 = set.resolve(None, None, Some("builtin-json"));
        assert_eq!(e2.unimportant_elements.len(), 2, "未绑定格式沿用会话层");
        let e3 = set.resolve(None, None, None);
        assert_eq!(e3.unimportant_elements.len(), 2, "无格式沿用会话层");
    }

    #[test]
    fn r11_set_layer_write() {
        let mut set = RuleSet::default();
        set.set_layer(
            RuleTarget::TypeDefault("folder-compare".into()),
            RuleLayer { compare: Some(CompareRules::default()), unimportant_elements: Some(vec!["注释".into()]) },
        );
        assert!(set.type_defaults.contains_key("folder-compare"));
        set.set_layer(RuleTarget::Session("s9".into()), RuleLayer::default());
        assert!(set.sessions.contains_key("s9"));
    }

    // ---- 5. 持久化 roundtrip（rules.json / formats.json） ----

    #[test]
    fn r12_rules_document_roundtrip() {
        let mut set = RuleSet::default();
        set.global = RuleLayer {
            compare: Some(CompareRules { case_unimportant: true, ..CompareRules::default() }),
            unimportant_elements: Some(vec!["注释".into()]),
        };
        set.type_defaults.insert(
            "text-compare".into(),
            RuleLayer { compare: None, unimportant_elements: Some(vec!["字符串".into()]) },
        );
        set.sessions.insert("s1".into(), RuleLayer::default());
        set.format_bindings.push(RuleFormatBinding {
            format_id: "builtin-cpp".into(),
            unimportant_elements: vec!["注释".into()],
        });

        let doc = set.to_document();
        let json = doc.to_json();
        let back = RulesDocument::from_json(&json).expect("rules.json 解析");
        assert_eq!(back, doc);
        let set2 = RuleSet::from_document(&back);
        assert_eq!(set2, set);
        // 解析行为不变
        assert_eq!(set2.resolve(Some("text-compare"), Some("s1"), None), set.resolve(Some("text-compare"), Some("s1"), None));
    }

    #[test]
    fn r13_rules_document_version_and_empty_doc() {
        let doc: RulesDocument = serde_json::from_str("{}").unwrap();
        assert_eq!(doc.version, 0, "缺省 0（旧文档无版本字段）");
        assert_eq!(doc.global, RuleLayer::default());
    }

    #[test]
    fn r14_formats_document_roundtrip() {
        let doc = FormatsDocument {
            version: FORMATS_DOCUMENT_VERSION,
            custom: vec![crate::formats::FileFormat::text("my-format", "自定义", &["*.my"], vec![])],
            builtin_overrides: vec![FormatOverride {
                id: "builtin-cpp".into(),
                enabled: Some(false),
                masks: Some(vec!["*.cpp".into()]),
                order: Some(3),
            }],
        };
        let json = doc.to_json();
        let back = FormatsDocument::from_json(&json).expect("formats.json 解析");
        assert_eq!(back, doc);
    }

    // ---- 6. RulesOptions 行为 ----

    #[test]
    fn r15_rules_options_defaults_inert() {
        let o = RulesOptions::default();
        assert!(!o.has_folding());
        assert_eq!(o.fold_line("// comment", None), "// comment", "缺省不折叠");
    }

    #[test]
    fn r16_rules_options_resolve_grammar_from_format() {
        let effective = EffectiveRules {
            compare: CompareRules::default(),
            unimportant_elements: vec!["注释".into()],
        };
        let o = RulesOptions::resolve(&effective, Some(c_format()));
        assert!(o.has_folding());
        assert_eq!(o.fold_line("// x", None), "\u{2}");
        // 无格式 → 无语法 → 不折叠
        let o2 = RulesOptions::resolve(&effective, None);
        assert!(!o2.has_folding());
        // 未引用任何元素 → 不折叠
        let empty = EffectiveRules::default();
        let o3 = RulesOptions::resolve(&empty, Some(c_format()));
        assert!(!o3.has_folding());
    }

    #[test]
    fn r17_compare_rules_default_matches_importance_default() {
        let imp: ImportanceOptions = CompareRules::default().into();
        assert_eq!(imp, ImportanceOptions::default(), "CompareRules 缺省与 P1 重要性缺省一致");
    }

    #[test]
    fn r18_text_diff_options_rules_default_backward_compatible() {
        let opts = TextDiffOptions::default();
        assert!(!opts.rules.has_folding(), "新字段缺省惰性：与 P0/P1 行为一致");
        // serde 场景（协议 DTO 独立）不适用；此处锚定 Rust 侧默认值等价性
        let opts2 = TextDiffOptions { whitespace: WhitespaceMode::IgnoreAll, ..TextDiffOptions::default() };
        assert!(!opts2.rules.has_folding());
    }
}
