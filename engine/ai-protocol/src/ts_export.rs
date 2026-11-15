//! TypeScript 类型导出管线（docs/03 §3.1.12 M23：ts-rs 生成 + 同步到 UI 侧）。
//!
//! - `#[ts(export, export_to = "generated/")]`：ts-rs 在 `cargo test` 时把
//!   `<Type>.ts` 写到本 crate 的 `generated/` 目录；
//! - [`export_all_to_ui`]：显式逐类型触发导出，并把 `generated/*.ts` 拷贝到
//!   仓库根 `ui/src/protocol/generated/`（UI 线消费；协议演进只加不改不删）。
//!
//! 入口测试：`generated_bindings_match_ui`（clean-tree 漂移校验，不写生产树）；
//! 再生成走 `cargo test -p ai-protocol -- --ignored regenerate_ui_bindings`。

use std::path::PathBuf;
use ts_rs::TS;

/// 仓库根 ui/src/protocol/generated/
fn ui_generated_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../ui/src/protocol/generated")
}

/// 逐类型导出到 ts-rs 的 `bindings/` scratch 目录（gitignored，不碰生产树）。
///
/// 注：`TS::export()` 写入 ts-rs 默认的 `bindings/` 目录（`export_to` 属性
/// 只影响其隐藏导出测试）。
fn export_to_bindings() -> Result<(), Box<dyn std::error::Error>> {
    // 先清空 scratch：ts-rs 只覆盖「本次实际导出」的类型，若不清空，
    // 上次生成的残留 .ts 会让「类型有 #[ts(export)] 但漏出显式列表」永远
    // 无法被发现（R3-21 实测：PictureOpenResult 等 10 个类型因此长期停在
    // 旧的 bigint 版本，而 drift 的逐文件比对完全看不出来）。
    let scratch = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("bindings");
    if scratch.is_dir() {
        for entry in std::fs::read_dir(&scratch)? {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) == Some("ts") {
                std::fs::remove_file(path)?;
            }
        }
    }
    // 逐类型显式导出（顺序确定，不依赖 ts-rs 生成的隐藏测试的执行顺序）
    use crate::types::*;
    RequestId::export()?;
    EntryKind::export()?;
    SideInfo::export()?;
    CompareCriteria::export()?;
    ContentCompareMode::export()?;
    ScanState::export()?;
    LineRange::export()?;
    CharRange::export()?;
    ClientInfo::export()?;
    InitializeParams::export()?;
    EngineCapabilities::export()?;
    LicenseCapabilities::export()?;
    InitializeResult::export()?;
    EngineInfo::export()?;
    DirOpenParams::export()?;
    DirOpenResult::export()?;
    DirEntryStatus::export()?;
    DirRow::export()?;
    DirStats::export()?;
    ScanProgress::export()?;
    BatchInfo::export()?;
    DirDelta::export()?;
    LineAlignAlgo::export()?;
    WhitespaceMode::export()?;
    TextDiffOptions::export()?;
    TextOpenParams::export()?;
    DiffSummary::export()?;
    TextOpenResult::export()?;
    SectionKind::export()?;
    Importance::export()?;
    InlineState::export()?;
    Section::export()?;
    TextSectionsParams::export()?;
    TextSectionsResult::export()?;
    TextSectionsNotification::export()?;
    SessionCloseParams::export()?;
    CancelParams::export()?;
    // P3 新增
    DiffSide::export()?;
    TextLinesParams::export()?;
    TextLinesResult::export()?;
    LineKeysEntry::export()?;
    TextInlineDiffParams::export()?;
    TextInlineDiffResult::export()?;
    InlineSpanKind::export()?;
    InlineSpanDto::export()?;
    InlineLineDiffDto::export()?;
    TextSectionEventKind::export()?;
    SectionIdSpan::export()?;
    ImportanceOptionsSpec::export()?;
    TextAlignAnchor::export()?;
    SideLineRange::export()?;
    TextManualAlignSpec::export()?;
    // R3-18 新增（text/edit 编辑增量重 diff）
    TextEditOp::export()?;
    TextEditParams::export()?;
    TextEditResult::export()?;
    // R3-04 新增（text/save 编辑内容落盘）
    TextSaveSide::export()?;
    TextSaveParams::export()?;
    TextSaveEntry::export()?;
    TextSaveResult::export()?;
    // P7 新增（text/merge3/* 三向合并）
    MergeChunkOrigin::export()?;
    MergeConflictKind::export()?;
    MergeFavorSide::export()?;
    MergeConflictDefault::export()?;
    MergeMarkerStyle::export()?;
    Merge3OptionsSpec::export()?;
    MergeOpenParams::export()?;
    MergeOpenResult::export()?;
    MergeSectionsParams::export()?;
    MergeConflictRowDto::export()?;
    MergeDecisionKind::export()?;
    MergeChunkDto::export()?;
    MergeSectionsResult::export()?;
    MergeDecisionDto::export()?;
    MergeDecideParams::export()?;
    MergeDecideResult::export()?;
    MergeOutputMode::export()?;
    MergeOutputParams::export()?;
    MergeOutputResult::export()?;
    MergeSaveParams::export()?;
    MergeSaveResult::export()?;
    // P9 新增（table/hex/patch + dir/execute）
    TableColumnAlign::export()?;
    TableRowAlign::export()?;
    TableOptionsSpec::export()?;
    TableOpenParams::export()?;
    TableColumnType::export()?;
    TableColumnInfo::export()?;
    TableSummary::export()?;
    TableOpenResult::export()?;
    TableSectionsParams::export()?;
    TableRowKind::export()?;
    TableCellStatus::export()?;
    TableCellDto::export()?;
    TableRowDto::export()?;
    TableSectionsResult::export()?;
    HexAlignMode::export()?;
    HexOptionsSpec::export()?;
    HexOpenParams::export()?;
    HexSummary::export()?;
    HexOpenResult::export()?;
    HexBytesParams::export()?;
    HexLineDto::export()?;
    HexBytesResult::export()?;
    PatchApplyParams::export()?;
    PatchFileStatus::export()?;
    PatchFileResultDto::export()?;
    PatchApplyResult::export()?;
    DirExecuteOpKind::export()?;
    DirExecuteOp::export()?;
    DirExecuteParams::export()?;
    DirExecuteOpStatus::export()?;
    DirExecuteOpResult::export()?;
    DirExecuteResult::export()?;
    // R3-07：文件夹三向合并（dir/mergeScan|mergePlan|mergeExecute）
    DirMergeCategory::export()?;
    DirMergeRowState::export()?;
    DirMergeRow::export()?;
    DirMergeScanStats::export()?;
    DirMergeScanParams::export()?;
    DirMergeScanResult::export()?;
    DirMergeTarget::export()?;
    DirMergeDecision::export()?;
    DirMergeOpKind::export()?;
    DirMergePlannedOp::export()?;
    DirMergePlanStats::export()?;
    DirMergePlanParams::export()?;
    DirMergePlanResult::export()?;
    DirMergeExecKind::export()?;
    DirMergeExecStatus::export()?;
    DirMergeExecDetail::export()?;
    DirMergeExecuteParams::export()?;
    DirMergeExecuteResult::export()?;
    // P2 新增
    NameFilterSpec::export()?;
    OtherFilterSpec::export()?;
    DisplayFilterSpec::export()?;
    ManualAlignPair::export()?;
    DirExpandParams::export()?;
    DirExpandResult::export()?;
    DirFilterParams::export()?;
    DirFilterResult::export()?;
    DirAlignParams::export()?;
    DirAlignResult::export()?;
    SyncPreset::export()?;
    PlannedOpKind::export()?;
    RuleSide::export()?;
    RuleAction::export()?;
    SyncRule::export()?;
    DirOperateParams::export()?;
    PlannedOperation::export()?;
    DirOperateResult::export()?;
    ProgressPhase::export()?;
    DirProgressEvent::export()?;
    // P10 新增（formats/rules + 能力位 + 会话 rules 载荷）
    GrammarItemKind::export()?;
    GrammarItemSpec::export()?;
    GrammarElementSpec::export()?;
    FormatViewKind::export()?;
    FileFormatSpec::export()?;
    FormatPackSpec::export()?;
    FormatsListResult::export()?;
    FormatsSaveParams::export()?;
    FormatsSaveResult::export()?;
    FormatsLoadParams::export()?;
    FormatsLoadResult::export()?;
    CompareRulesSpec::export()?;
    RuleLayerSpec::export()?;
    RuleFormatBindingSpec::export()?;
    EffectiveRulesSpec::export()?;
    RuleScope::export()?;
    RulesGetParams::export()?;
    RulesGetResult::export()?;
    RulesSetParams::export()?;
    RulesSetResult::export()?;
    RulesSpec::export()?;
    DetectedFileKind::export()?;
    // P11 新增（report/generate 布局族 + 能力位 reports）
    ReportLayout::export()?;
    ReportDisplayFilter::export()?;
    ReportDirDisplayFilter::export()?;
    ReportPatchStyle::export()?;
    ReportColorMode::export()?;
    ReportWrapMode::export()?;
    ReportDirColumns::export()?;
    ReportGenerateParams::export()?;
    ReportGenerateResult::export()?;
    crate::error::RpcErrorObject::export()?;
    // R3-21：以下 10 个类型此前有 #[ts(export)] 却漏出显式列表，导致其
    // UI 绑定从未被重新生成（PictureOpenResult/RpcErrorObject 长期停在旧的
    // bigint 版本）。补齐后由 generated_bindings_match_ui 的反向自检锁定。
    ContentCompareMethod::export()?;
    ContentCompareResult::export()?;
    DirCompareContentParams::export()?;
    DirCompareContentResult::export()?;
    DirContentCompareRow::export()?;
    DirScanIssue::export()?;
    PictureBlockDto::export()?;
    PictureOpenParams::export()?;
    PictureOpenResult::export()?;
    PictureOptionsSpec::export()?;
    Ok(())
}

/// 导出全部协议类型并同步到 UI 侧 tracked 目录，返回生成的文件数。
/// 仅手动再生成入口（`regenerate_ui_bindings`，#[ignore]）调用；普通测试不写生产树。
pub fn export_all_to_ui() -> Result<usize, Box<dyn std::error::Error>> {
    export_to_bindings()?;
    let bindings = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("bindings");
    let ui_dir = ui_generated_dir();
    std::fs::create_dir_all(&ui_dir)?;
    let mut count = 0usize;
    for entry in std::fs::read_dir(&bindings)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("ts") {
            let dest = ui_dir.join(entry.file_name());
            std::fs::copy(&path, &dest)?;
            count += 1;
        }
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// clean-tree 漂移校验：导出到 bindings/ scratch 后与 tracked UI generated 逐字节比对。
    /// 不写生产树；rust 类型改了但没再生成 UI 契约时这里变红。
    #[test]
    fn generated_bindings_match_ui() {
        export_to_bindings().expect("TS 导出失败");
        let bindings = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("bindings");
        let ui_dir = ui_generated_dir();
        let mut n = 0usize;
        for entry in std::fs::read_dir(&bindings).expect("bindings 目录") {
            let entry = entry.expect("read_dir");
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("ts") {
                continue;
            }
            let name = entry.file_name();
            let label = name.to_string_lossy();
            let fresh = std::fs::read_to_string(&path).expect("读 bindings");
            let old = std::fs::read_to_string(ui_dir.join(&name))
                .unwrap_or_else(|_| panic!("UI generated 缺 {label}（需再生成）"));
            assert_eq!(fresh, old, "{label} 与 UI generated 漂移（需再生成）");
            n += 1;
        }
        // 反向自检：UI generated 里的每个 .ts 都必须由本次导出重新生成。
        // 否则「类型有 #[ts(export)] 但忘了进 export_to_bindings 列表」会让
        // 绑定永久陈旧，而逐文件比对完全发现不了（R3-21 实测踩中）。
        for entry in std::fs::read_dir(&ui_dir).expect("UI generated 目录") {
            let name = entry.expect("read_dir").file_name();
            if name.to_string_lossy().ends_with(".ts") {
                assert!(
                    bindings.join(&name).is_file(),
                    "UI generated 有 {} 但 bindings 未生成：该类型漏出 export_to_bindings 列表",
                    name.to_string_lossy()
                );
            }
        }
        // R3-21：生成类型不得再出现 bigint —— 线上 NDJSON 是 JSON number，
        // bigint 会让 UI 写出 `as unknown as bigint` 之类的双重 cast。
        for entry in std::fs::read_dir(&ui_dir).expect("UI generated 目录") {
            let path = entry.expect("read_dir").path();
            if path.extension().and_then(|e| e.to_str()) != Some("ts") {
                continue;
            }
            let content = std::fs::read_to_string(&path).expect("读 UI generated");
            assert!(
                !content.contains("bigint"),
                "{} 仍生成 bigint（与 NDJSON number 不符）",
                path.display()
            );
        }
        let ui_count = std::fs::read_dir(&ui_dir)
            .expect("UI generated 目录")
            .filter(|e| e.as_ref().is_ok_and(|e| e.file_name().to_string_lossy().ends_with(".ts")))
            .count();
        assert_eq!(n, ui_count, "导出类型数应与 UI generated 文件数一致");
    }

    /// 手动再生成 UI 契约：cargo test -p ai-protocol -- --ignored regenerate_ui_bindings
    #[test]
    #[ignore]
    fn regenerate_ui_bindings() {
        let n = export_all_to_ui().expect("TS 导出失败");
        assert!(n >= 139, "应导出全部协议类型，实际 {n}");
    }
}
