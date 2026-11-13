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
        importance: ai_core::importance_key(entry, opts, imp).into_owned(),
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
