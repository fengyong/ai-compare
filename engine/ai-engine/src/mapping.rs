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
}
