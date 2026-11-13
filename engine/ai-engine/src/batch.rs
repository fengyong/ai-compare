//! dir/delta 分批（batch + has_more，docs/03 §2.3.2 批量与背压）。

use ai_protocol::{BatchInfo, DirDelta, DirRow, DirStats};

/// 默认单批最大行数（协议能力上报一致）
pub const DEFAULT_MAX_BATCH_ROWS: usize = 512;

/// 一个批次：行切片 + 是否还有后续
#[derive(Debug, Clone, PartialEq)]
pub struct Batch {
    pub index: u32,
    pub has_more: bool,
    pub upserts: Vec<DirRow>,
}

/// 把行序列切为批次（`max == 0` 视为不分批、单批全量）。
/// 空行集产出单个空批次（保证客户端收到 stats 终态）。
pub fn batch_rows(rows: Vec<DirRow>, max: usize) -> Vec<Batch> {
    let mut out = Vec::new();
    let total = rows.len();
    if total == 0 {
        out.push(Batch { index: 0, has_more: false, upserts: Vec::new() });
        return out;
    }
    let max = if max == 0 { usize::MAX } else { max };
    let mut start = 0usize;
    let mut index = 0u32;
    while start < total {
        let end = (start + max).min(total);
        let has_more = end < total;
        out.push(Batch { index, has_more, upserts: rows[start..end].to_vec() });
        index += 1;
        start = end;
    }
    out
}

/// 组装 dir/delta 通知载荷
pub fn make_delta(
    session_id: &str,
    rev: u64,
    batch: &Batch,
    stats: DirStats,
) -> DirDelta {
    make_delta_with_progress(session_id, rev, batch, stats, None, Vec::new())
}

/// 组装 dir/delta 通知载荷（带进度与移除集；P2 惰性扫描路径）
pub fn make_delta_with_progress(
    session_id: &str,
    rev: u64,
    batch: &Batch,
    stats: DirStats,
    progress: Option<ai_protocol::ScanProgress>,
    removals: Vec<String>,
) -> DirDelta {
    DirDelta {
        session_id: session_id.to_string(),
        rev,
        upserts: batch.upserts.clone(),
        removals,
        stats,
        progress,
        batch: BatchInfo { index: batch.index, has_more: batch.has_more },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ai_protocol::{DirEntryStatus, EntryKind};

    fn row(i: usize) -> DirRow {
        DirRow {
            id: format!("f{i}.txt"),
            name: format!("f{i}.txt"),
            kind: EntryKind::File,
            left: None,
            right: None,
            status: DirEntryStatus::RightOnly,
            has_children: false,
            hidden: false,
            suppressed: false,
        }
    }

    #[test]
    fn splits_with_has_more_flags() {
        let rows: Vec<DirRow> = (0..5).map(row).collect();
        let batches = batch_rows(rows, 2);
        assert_eq!(batches.len(), 3);
        assert_eq!(batches[0].upserts.len(), 2);
        assert!(batches[0].has_more);
        assert_eq!(batches[1].upserts.len(), 2);
        assert!(batches[1].has_more);
        assert_eq!(batches[2].upserts.len(), 1);
        assert!(!batches[2].has_more);
        assert_eq!(batches[2].index, 2);
    }

    #[test]
    fn empty_rows_single_batch() {
        let batches = batch_rows(Vec::new(), 100);
        assert_eq!(batches.len(), 1);
        assert!(!batches[0].has_more);
        assert!(batches[0].upserts.is_empty());
    }

    #[test]
    fn zero_max_is_single_batch() {
        let rows: Vec<DirRow> = (0..3).map(row).collect();
        let batches = batch_rows(rows, 0);
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].upserts.len(), 3);
    }

    #[test]
    fn delta_payload_shape() {
        let rows: Vec<DirRow> = (0..1).map(row).collect();
        let batch = &batch_rows(rows, 10)[0];
        let delta = make_delta("s1", 1, batch, DirStats::default());
        let v = serde_json::to_value(&delta).unwrap();
        assert_eq!(v["sessionId"], "s1");
        assert_eq!(v["rev"], 1);
        assert_eq!(v["batch"]["index"], 0);
        assert_eq!(v["batch"]["hasMore"], false);
        assert!(v.get("progress").is_none());
        assert!(v.get("removals").is_some());
    }
}
