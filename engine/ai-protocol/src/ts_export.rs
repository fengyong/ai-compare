//! TypeScript 类型导出管线（docs/03 §3.1.12 M23：ts-rs 生成 + 同步到 UI 侧）。
//!
//! - `#[ts(export, export_to = "generated/")]`：ts-rs 在 `cargo test` 时把
//!   `<Type>.ts` 写到本 crate 的 `generated/` 目录；
//! - [`export_all_to_ui`]：显式逐类型触发导出，并把 `generated/*.ts` 拷贝到
//!   仓库根 `ui/src/protocol/generated/`（UI 线消费；协议演进只加不改不删）。
//!
//! 入口测试：`export_ts_bindings`（`cargo test -p ai-protocol` 即更新生成物）。

use std::path::PathBuf;
use ts_rs::TS;

/// 仓库根 ui/src/protocol/generated/
fn ui_generated_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../ui/src/protocol/generated")
}

/// 导出全部协议类型并同步到 UI 侧目录，返回生成的文件数。
///
/// 注：`TS::export()` 写入 ts-rs 默认的 `bindings/` 目录（`export_to` 属性
/// 只影响其隐藏导出测试）；此处显式逐类型导出后统一拷贝到 UI 侧。
pub fn export_all_to_ui() -> Result<usize, Box<dyn std::error::Error>> {
    // 1. 逐类型显式导出（顺序确定，不依赖 ts-rs 生成的隐藏测试的执行顺序）
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
    crate::error::RpcErrorObject::export()?;

    // 2. 拷贝到 UI 侧
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

    /// 生成/更新 ui/src/protocol/generated/*.ts（UI 线的类型契约）
    #[test]
    fn export_ts_bindings() {
        let n = export_all_to_ui().expect("TS 导出失败");
        assert!(n >= 77, "应导出全部协议类型（P7 起含 text/merge3 全家），实际 {n}");
        let ui_dir = ui_generated_dir();
        assert!(ui_dir.join("DirDelta.ts").exists(), "DirDelta.ts 应存在");
        assert!(ui_dir.join("Section.ts").exists(), "Section.ts 应存在");
    }
}
