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
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../ui/src/protocol/generated")
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
    let cfg = ts_rs::Config::from_env();
    RequestId::export(&cfg)?;
    EntryKind::export(&cfg)?;
    SideInfo::export(&cfg)?;
    CompareCriteria::export(&cfg)?;
    ContentCompareMode::export(&cfg)?;
    ScanState::export(&cfg)?;
    LineRange::export(&cfg)?;
    CharRange::export(&cfg)?;
    ClientInfo::export(&cfg)?;
    InitializeParams::export(&cfg)?;
    EngineCapabilities::export(&cfg)?;
    LicenseCapabilities::export(&cfg)?;
    InitializeResult::export(&cfg)?;
    EngineInfo::export(&cfg)?;
    DirOpenParams::export(&cfg)?;
    DirOpenResult::export(&cfg)?;
    DirEntryStatus::export(&cfg)?;
    DirRow::export(&cfg)?;
    DirStats::export(&cfg)?;
    ScanProgress::export(&cfg)?;
    BatchInfo::export(&cfg)?;
    DirDelta::export(&cfg)?;
    LineAlignAlgo::export(&cfg)?;
    WhitespaceMode::export(&cfg)?;
    TextDiffOptions::export(&cfg)?;
    TextOpenParams::export(&cfg)?;
    DiffSummary::export(&cfg)?;
    TextOpenResult::export(&cfg)?;
    SectionKind::export(&cfg)?;
    Importance::export(&cfg)?;
    InlineState::export(&cfg)?;
    Section::export(&cfg)?;
    TextSectionsParams::export(&cfg)?;
    TextSectionsResult::export(&cfg)?;
    TextSectionsNotification::export(&cfg)?;
    SessionCloseParams::export(&cfg)?;
    CancelParams::export(&cfg)?;
    // P3 新增
    DiffSide::export(&cfg)?;
    TextLinesParams::export(&cfg)?;
    TextLinesResult::export(&cfg)?;
    LineKeysEntry::export(&cfg)?;
    TextInlineDiffParams::export(&cfg)?;
    TextInlineDiffResult::export(&cfg)?;
    InlineSpanKind::export(&cfg)?;
    InlineSpanDto::export(&cfg)?;
    InlineLineDiffDto::export(&cfg)?;
    TextSectionEventKind::export(&cfg)?;
    SectionIdSpan::export(&cfg)?;
    ImportanceOptionsSpec::export(&cfg)?;
    TextAlignAnchor::export(&cfg)?;
    SideLineRange::export(&cfg)?;
    TextManualAlignSpec::export(&cfg)?;
    // R3-18 新增（text/edit 编辑增量重 diff）
    TextEditOp::export(&cfg)?;
    TextEditParams::export(&cfg)?;
    TextEditResult::export(&cfg)?;
    // R3-04 新增（text/save 编辑内容落盘）
    TextSaveSide::export(&cfg)?;
    TextSaveParams::export(&cfg)?;
    TextSaveEntry::export(&cfg)?;
    TextSaveResult::export(&cfg)?;
    // P7 新增（text/merge3/* 三向合并）
    MergeChunkOrigin::export(&cfg)?;
    MergeConflictKind::export(&cfg)?;
    MergeFavorSide::export(&cfg)?;
    MergeConflictDefault::export(&cfg)?;
    MergeMarkerStyle::export(&cfg)?;
    Merge3OptionsSpec::export(&cfg)?;
    MergeOpenParams::export(&cfg)?;
    MergeOpenResult::export(&cfg)?;
    MergeSectionsParams::export(&cfg)?;
    MergeConflictRowDto::export(&cfg)?;
    MergeDecisionKind::export(&cfg)?;
    MergeChunkDto::export(&cfg)?;
    MergeSectionsResult::export(&cfg)?;
    MergeDecisionDto::export(&cfg)?;
    MergeDecideParams::export(&cfg)?;
    MergeDecideResult::export(&cfg)?;
    MergeOutputMode::export(&cfg)?;
    MergeOutputParams::export(&cfg)?;
    MergeOutputResult::export(&cfg)?;
    MergeSaveParams::export(&cfg)?;
    MergeSaveResult::export(&cfg)?;
    // P9 新增（table/hex/patch + dir/execute）
    TableColumnAlign::export(&cfg)?;
    TableRowAlign::export(&cfg)?;
    TableOptionsSpec::export(&cfg)?;
    TableOpenParams::export(&cfg)?;
    TableColumnType::export(&cfg)?;
    TableColumnInfo::export(&cfg)?;
    TableSummary::export(&cfg)?;
    TableOpenResult::export(&cfg)?;
    TableSectionsParams::export(&cfg)?;
    TableRowKind::export(&cfg)?;
    TableCellStatus::export(&cfg)?;
    TableCellDto::export(&cfg)?;
    TableRowDto::export(&cfg)?;
    TableSectionsResult::export(&cfg)?;
    HexAlignMode::export(&cfg)?;
    HexOptionsSpec::export(&cfg)?;
    HexOpenParams::export(&cfg)?;
    HexSummary::export(&cfg)?;
    HexOpenResult::export(&cfg)?;
    HexBytesParams::export(&cfg)?;
    HexLineDto::export(&cfg)?;
    HexBytesResult::export(&cfg)?;
    PatchApplyParams::export(&cfg)?;
    PatchFileStatus::export(&cfg)?;
    PatchFileResultDto::export(&cfg)?;
    PatchApplyResult::export(&cfg)?;
    DirExecuteOpKind::export(&cfg)?;
    DirExecuteOp::export(&cfg)?;
    DirExecuteParams::export(&cfg)?;
    DirExecuteOpStatus::export(&cfg)?;
    DirExecuteOpResult::export(&cfg)?;
    DirExecuteResult::export(&cfg)?;
    // R3-07：文件夹三向合并（dir/mergeScan|mergePlan|mergeExecute）
    DirMergeCategory::export(&cfg)?;
    DirMergeRowState::export(&cfg)?;
    DirMergeRow::export(&cfg)?;
    DirMergeScanStats::export(&cfg)?;
    DirMergeScanParams::export(&cfg)?;
    DirMergeScanResult::export(&cfg)?;
    DirMergeTarget::export(&cfg)?;
    DirMergeDecision::export(&cfg)?;
    DirMergeOpKind::export(&cfg)?;
    DirMergePlannedOp::export(&cfg)?;
    DirMergePlanStats::export(&cfg)?;
    DirMergePlanParams::export(&cfg)?;
    DirMergePlanResult::export(&cfg)?;
    DirMergeExecKind::export(&cfg)?;
    DirMergeExecStatus::export(&cfg)?;
    DirMergeExecDetail::export(&cfg)?;
    DirMergeExecuteParams::export(&cfg)?;
    DirMergeExecuteResult::export(&cfg)?;
    // P2 新增
    NameFilterSpec::export(&cfg)?;
    OtherFilterSpec::export(&cfg)?;
    DisplayFilterSpec::export(&cfg)?;
    ManualAlignPair::export(&cfg)?;
    DirExpandParams::export(&cfg)?;
    DirExpandResult::export(&cfg)?;
    DirFilterParams::export(&cfg)?;
    DirFilterResult::export(&cfg)?;
    DirAlignParams::export(&cfg)?;
    DirAlignResult::export(&cfg)?;
    SyncPreset::export(&cfg)?;
    PlannedOpKind::export(&cfg)?;
    RuleSide::export(&cfg)?;
    RuleAction::export(&cfg)?;
    SyncRule::export(&cfg)?;
    DirOperateParams::export(&cfg)?;
    PlannedOperation::export(&cfg)?;
    DirOperateResult::export(&cfg)?;
    ProgressPhase::export(&cfg)?;
    DirProgressEvent::export(&cfg)?;
    // P10 新增（formats/rules + 能力位 + 会话 rules 载荷）
    GrammarItemKind::export(&cfg)?;
    GrammarItemSpec::export(&cfg)?;
    GrammarElementSpec::export(&cfg)?;
    FormatViewKind::export(&cfg)?;
    FileFormatSpec::export(&cfg)?;
    FormatPackSpec::export(&cfg)?;
    FormatsListResult::export(&cfg)?;
    FormatsSaveParams::export(&cfg)?;
    FormatsSaveResult::export(&cfg)?;
    FormatsLoadParams::export(&cfg)?;
    FormatsLoadResult::export(&cfg)?;
    CompareRulesSpec::export(&cfg)?;
    RuleLayerSpec::export(&cfg)?;
    RuleFormatBindingSpec::export(&cfg)?;
    EffectiveRulesSpec::export(&cfg)?;
    RuleScope::export(&cfg)?;
    RulesGetParams::export(&cfg)?;
    RulesGetResult::export(&cfg)?;
    RulesSetParams::export(&cfg)?;
    RulesSetResult::export(&cfg)?;
    RulesSpec::export(&cfg)?;
    DetectedFileKind::export(&cfg)?;
    // P11 新增（report/generate 布局族 + 能力位 reports）
    ReportLayout::export(&cfg)?;
    ReportDisplayFilter::export(&cfg)?;
    ReportDirDisplayFilter::export(&cfg)?;
    ReportPatchStyle::export(&cfg)?;
    ReportColorMode::export(&cfg)?;
    ReportWrapMode::export(&cfg)?;
    ReportDirColumns::export(&cfg)?;
    ReportGenerateParams::export(&cfg)?;
    ReportGenerateResult::export(&cfg)?;
    crate::error::RpcErrorObject::export(&cfg)?;
    // R3-21：以下 10 个类型此前有 #[ts(export)] 却漏出显式列表，导致其
    // UI 绑定从未被重新生成（PictureOpenResult/RpcErrorObject 长期停在旧的
    // bigint 版本）。补齐后由 generated_bindings_match_ui 的反向自检锁定。
    ContentCompareMethod::export(&cfg)?;
    ContentCompareResult::export(&cfg)?;
    DirCompareContentParams::export(&cfg)?;
    DirCompareContentResult::export(&cfg)?;
    DirContentCompareRow::export(&cfg)?;
    DirScanIssue::export(&cfg)?;
    PictureBlockDto::export(&cfg)?;
    PictureOpenParams::export(&cfg)?;
    PictureOpenResult::export(&cfg)?;
    PictureOptionsSpec::export(&cfg)?;
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
            .filter(|e| {
                e.as_ref()
                    .is_ok_and(|e| e.file_name().to_string_lossy().ends_with(".ts"))
            })
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
