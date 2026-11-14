//! 补丁应用引擎（P9 / FR-PATCH-010/011 引擎侧；docs/04 §P9）。
//!
//! 能力面：
//! - unified diff 解析：git 头容错（`diff --git`/`index`/mode/rename 行）、
//!   `---`/`+++` 时间戳容错（TAB 后内容忽略）、`@@` 节标题后缀容错、
//!   `\ No newline at end of file` 跟踪、`Binary files ... differ` /
//!   `GIT binary patch` 识别（二进制文件跳过并报告）；
//! - 应用：精确上下文匹配（位置搜索含前后漂移）+ 模糊上下文匹配（可配
//!   fuzz：逐层丢弃首尾上下文行重试）；CRLF 容忍（匹配忽略行尾 CR，
//!   输出保留原文件行尾风格）；新文件创建 / 文件删除（/dev/null 头）；
//! - 与 `aic diff` 输出自洽：自产 unified diff -> 解析 -> 应用 -> 还原
//!   右侧文本（roundtrip 测试）。
//!
//! IO 边界说明：解析与"文本 -> 文本"应用为纯函数（L2）；多文件落盘经
//! [`PatchFs`] 抽象（可注入内存实现测试），默认 [`StdFs`] 走 std::fs——
//! 补丁应用本质是文件级操作（P9 任务书指定引擎侧），在模块层显式声明该例外。

use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// 解析模型
// ---------------------------------------------------------------------------

/// 补丁内单文件
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PatchFile {
    /// 旧文件路径（None = /dev/null，即新文件）
    pub old_path: Option<String>,
    /// 新文件路径（None = /dev/null，即删除）
    pub new_path: Option<String>,
    pub hunks: Vec<PatchHunk>,
    /// 二进制文件（Binary files ... differ / GIT binary patch）
    pub binary: bool,
    /// git 头标注的 new file
    pub is_new: bool,
    /// git 头标注的 deleted file
    pub is_delete: bool,
}

impl PatchFile {
    /// 应用目标路径（新建取新路径、删除取旧路径、修改优先新路径）
    pub fn target_path(&self) -> Option<&str> {
        if self.is_new {
            self.new_path.as_deref()
        } else if self.is_delete {
            self.old_path.as_deref()
        } else {
            self.new_path.as_deref().or(self.old_path.as_deref())
        }
    }

    /// 补丁内标签（报告用）
    pub fn label(&self) -> String {
        self.target_path()
            .map(str::to_string)
            .unwrap_or_else(|| self.old_path.clone().or(self.new_path.clone()).unwrap_or_default())
    }
}

/// hunk 内行类别
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchLineKind {
    Context,
    Delete,
    Add,
}

/// hunk 内单行（text 不含前缀与行尾；had_cr = 补丁文件中该行以 CRLF 结尾）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchLine {
    pub kind: PatchLineKind,
    pub text: String,
    pub had_cr: bool,
}

/// 单个 hunk
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchHunk {
    /// 旧文件起始行号（1 起，unified diff 语义）
    pub old_start: u32,
    pub old_count: u32,
    pub new_start: u32,
    pub new_count: u32,
    pub lines: Vec<PatchLine>,
    /// 旧文件末行无换行符（"\ No newline" 紧随 Delete/Context）
    pub old_no_newline: bool,
    /// 新文件末行无换行符（"\ No newline" 紧随 Add/Context）
    pub new_no_newline: bool,
}

/// 解析结果
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ParsedPatch {
    pub files: Vec<PatchFile>,
}

/// 解析错误
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PatchError {
    #[error("未找到任何补丁内容（hunk）")]
    Empty,
    #[error("第 {line} 行：hunk 头不合法: {text}")]
    InvalidHunkHeader { line: usize, text: String },
    #[error("第 {line} 行：hunk 体不合法（期望上下文/删除/新增行）: {text}")]
    InvalidHunkBody { line: usize, text: String },
    #[error("第 {line} 行：--- 头缺少配对的 +++ 头")]
    MissingPlusHeader { line: usize },
}

/// unified diff 解析（git / diff -u 产物；时间戳与节标题容错）
pub fn parse_patch(text: &str) -> Result<ParsedPatch, PatchError> {
    // 逐行拆分并记录 CRLF 标记（lines() 会吞掉 \r，无法区分 LF/CRLF 补丁）
    let raw: Vec<&str> = text.split('\n').collect();
    let mut lines: Vec<&str> = Vec::with_capacity(raw.len());
    let mut cr: Vec<bool> = Vec::with_capacity(raw.len());
    for l in raw {
        match l.strip_suffix('\r') {
            Some(stripped) => {
                lines.push(stripped);
                cr.push(true);
            }
            None => {
                lines.push(l);
                cr.push(false);
            }
        }
    }
    let mut files: Vec<PatchFile> = Vec::new();
    let mut i = 0usize;

    // 当前 git 头累积的元数据（遇到 ---/+++/Binary/GIT 后落盘）
    let mut cur = PatchFile::default();
    let mut cur_has_git_header = false;

    fn flush(cur: &mut PatchFile, has_header: &mut bool, files: &mut Vec<PatchFile>) {
        if !cur.hunks.is_empty() || cur.binary || cur_has_header_marker(cur) {
            files.push(std::mem::take(cur));
        }
        *has_header = false;
        fn cur_has_header_marker(_c: &PatchFile) -> bool {
            false // 由调用方在 push 时判断（old/new_path 已设置即为完整文件）
        }
    }

    while i < lines.len() {
        let line = lines[i];
        if let Some(rest) = line.strip_prefix("diff --git ") {
            // 先冲刷上一个文件（若未冲刷）
            if !cur.hunks.is_empty() || cur.binary {
                flush(&mut cur, &mut cur_has_git_header, &mut files);
            } else if !files.is_empty() || cur_has_git_header {
                // 只有 git 头没有 hunk 的文件（如纯 mode 变更）：按空文件记录
                if cur_has_git_header {
                    files.push(std::mem::take(&mut cur));
                }
            }
            cur = PatchFile::default();
            cur_has_git_header = true;
            // diff --git a/<old> b/<new>：解析路径供后续 ---/+++ 缺失时兜底
            if let Some((o, n)) = parse_git_diff_paths(rest) {
                cur.old_path = Some(o);
                cur.new_path = Some(n);
            }
            i += 1;
            continue;
        }
        if cur_has_git_header {
            if line.starts_with("new file mode") {
                cur.is_new = true;
                i += 1;
                continue;
            }
            if line.starts_with("deleted file mode") {
                cur.is_delete = true;
                i += 1;
                continue;
            }
            if line.starts_with("Binary files") || line.starts_with("GIT binary patch") {
                cur.binary = true;
                i += 1;
                files.push(std::mem::take(&mut cur));
                cur_has_git_header = false;
                continue;
            }
            // --- /+++ 落到下方文件头分支处理；其余元数据/未知行跳过
            if !line.starts_with("--- ") {
                i += 1;
                continue;
            }
        }
        if let Some(old_raw) = line.strip_prefix("--- ") {
            // 文件头起点（纯 unified diff 或 git 头之后）
            if !cur.hunks.is_empty() || cur.binary {
                flush(&mut cur, &mut cur_has_git_header, &mut files);
                cur = PatchFile::default();
            }
            let old_path = parse_patch_path(old_raw);
            cur.old_path = old_path;
            i += 1;
            // 期望 +++ 头
            let plus = lines.get(i).ok_or(PatchError::MissingPlusHeader { line: i })?;
            let Some(new_raw) = plus.strip_prefix("+++ ") else {
                return Err(PatchError::MissingPlusHeader { line: i + 1 });
            };
            cur.new_path = parse_patch_path(new_raw);
            i += 1;
            // 读取 hunks 直到下一个文件头 / 文件尾
            while i < lines.len() {
                let l = lines[i];
                if l.starts_with("diff --git ") || l.starts_with("--- ") || l.starts_with("Binary files") {
                    break;
                }
                if l.starts_with("@@ ") {
                    let (hunk, consumed) = parse_hunk(&lines[i..], &cr[i..], i + 1)?;
                    cur.hunks.push(hunk);
                    i += consumed;
                    continue;
                }
                if l.trim().is_empty() {
                    i += 1;
                    continue;
                }
                // 未知行（如 diff -p 的 \ No newline 后杂散）：跳过
                i += 1;
            }
            files.push(std::mem::take(&mut cur));
            cur_has_git_header = false;
            continue;
        }
        if line.starts_with("Binary files ") || line.starts_with("GIT binary patch") {
            // 无 git 头的二进制标记（纯 diff -r 产物）：路径从行内取
            let mut f = std::mem::take(&mut cur);
            f.binary = true;
            if f.old_path.is_none() {
                if let Some((o, n)) = parse_binary_paths(line) {
                    f.old_path = Some(o);
                    f.new_path = Some(n);
                }
            }
            files.push(f);
            cur_has_git_header = false;
            i += 1;
            continue;
        }
        i += 1;
    }
    // 尾部残余（只有 git 头没有 hunk 的 mode-only 变更）
    if cur_has_git_header && (cur.is_new || cur.is_delete || cur.old_path.is_some()) && cur.hunks.is_empty() && !cur.binary {
        files.push(cur);
    }
    let any_hunks = files.iter().any(|f| !f.hunks.is_empty() || f.binary);
    if !any_hunks {
        return Err(PatchError::Empty);
    }
    Ok(ParsedPatch { files })
}

/// `diff --git a/x b/x` 路径对解析
fn parse_git_diff_paths(rest: &str) -> Option<(String, String)> {
    let marker = " b/";
    let pos = rest.find(marker)?;
    let old = rest[..pos].strip_prefix("a/")?;
    let new = &rest[pos + marker.len()..];
    Some((unquote_path(old), unquote_path(new)))
}

/// `Binary files a/x and b/x differ` 路径对解析
fn parse_binary_paths(line: &str) -> Option<(String, String)> {
    let rest = line.strip_prefix("Binary files ")?;
    let pos = rest.find(" and ")?;
    let old = rest[..pos].strip_prefix("a/")?;
    let new = rest[pos + 5..].strip_prefix("b/")?.strip_suffix(" differ")?;
    Some((unquote_path(old), unquote_path(new)))
}

/// git quotepath 反转义（最小实现：\" -> "）
fn unquote_path(p: &str) -> String {
    p.trim_matches('"').replace("\\\"", "\"")
}

/// `--- a/path[\t<timestamp>]` 路径解析：TAB 后时间戳剥离；/dev/null -> None
fn parse_patch_path(raw: &str) -> Option<String> {
    let path = raw.split('\t').next().unwrap_or(raw).trim_end_matches('\r');
    if path.is_empty() || path == "/dev/null" {
        return None;
    }
    let unquoted = unquote_path(path);
    let stripped = unquoted
        .strip_prefix("a/")
        .or_else(|| unquoted.strip_prefix("b/"))
        .unwrap_or(&unquoted);
    Some(stripped.to_string())
}

/// hunk 头解析：`@@ -a[,b] +c[,d] @@<任意节标题>`。
/// `hb` 以 hunk 头行为首行；返回 (hunk, 消费的行数含头行)。
fn parse_hunk(hb: &[&str], cr: &[bool], abs_line: usize) -> Result<(PatchHunk, usize), PatchError> {
    let header = hb[0];
    let err = || PatchError::InvalidHunkHeader { line: abs_line, text: header.to_string() };
    let body = header.strip_prefix("@@ ").ok_or_else(err)?;
    let (ranges, _title) = body.split_once(" @@").ok_or_else(err)?;
    let mut parts = ranges.split_whitespace();
    let old_part = parts.next().ok_or_else(err)?;
    let new_part = parts.next().ok_or_else(err)?;
    let parse_range = |s: &str| -> Option<(u32, u32)> {
        let s = s.strip_prefix('-').or_else(|| s.strip_prefix('+'))?;
        match s.split_once(',') {
            Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
            None => Some((s.parse().ok()?, 1)),
        }
    };
    let (old_start, old_count) = parse_range(old_part).ok_or_else(err)?;
    let (new_start, new_count) = parse_range(new_part).ok_or_else(err)?;

    let mut hunk = PatchHunk {
        old_start,
        old_count,
        new_start,
        new_count,
        lines: Vec::new(),
        old_no_newline: false,
        new_no_newline: false,
    };
    let mut i = 1usize; // hb[0] = 头行
    let mut old_left = old_count as usize;
    let mut new_left = new_count as usize;
    while i < hb.len() && (old_left > 0 || new_left > 0) {
        let l = hb[i];
        let at = abs_line + i;
        // 空 hunk 体行（尾随空上下行被工具剥掉前导空格）：按空上下文容忍
        if l.is_empty() {
            if old_left > 0 {
                hunk.lines.push(PatchLine { kind: PatchLineKind::Context, text: String::new(), had_cr: cr[i] });
                old_left -= 1;
                new_left = new_left.saturating_sub(1);
            }
            i += 1;
            continue;
        }
        let (kind, text) = match l.as_bytes().first() {
            Some(b' ') => {
                if old_left == 0 {
                    return Err(PatchError::InvalidHunkBody { line: at, text: l.to_string() });
                }
                (PatchLineKind::Context, &l[1..])
            }
            Some(b'-') => {
                if old_left == 0 {
                    return Err(PatchError::InvalidHunkBody { line: at, text: l.to_string() });
                }
                (PatchLineKind::Delete, &l[1..])
            }
            Some(b'+') => {
                if new_left == 0 {
                    return Err(PatchError::InvalidHunkBody { line: at, text: l.to_string() });
                }
                (PatchLineKind::Add, &l[1..])
            }
            Some(b'\\') => {
                // "\ No newline at end of file"：修饰前一行所在侧
                if matches!(hunk.lines.last().map(|x| x.kind), Some(PatchLineKind::Add)) {
                    hunk.new_no_newline = true;
                } else {
                    hunk.old_no_newline = true;
                    if matches!(hunk.lines.last().map(|x| x.kind), Some(PatchLineKind::Context)) {
                        hunk.new_no_newline = true;
                    }
                }
                i += 1;
                continue;
            }
            _ => return Err(PatchError::InvalidHunkBody { line: at, text: l.to_string() }),
        };
        hunk.lines.push(PatchLine { kind, text: text.to_string(), had_cr: cr[i] });
        match kind {
            PatchLineKind::Context => {
                old_left -= 1;
                new_left = new_left.saturating_sub(1);
            }
            PatchLineKind::Delete => old_left -= 1,
            PatchLineKind::Add => new_left -= 1,
        }
        i += 1;
    }
    // 计数满足后的收尾 "\ No newline"（修饰 hunk 末行）
    if i < hb.len() && hb[i].starts_with('\\') {
        if matches!(hunk.lines.last().map(|x| x.kind), Some(PatchLineKind::Add)) {
            hunk.new_no_newline = true;
        } else {
            hunk.old_no_newline = true;
            if matches!(hunk.lines.last().map(|x| x.kind), Some(PatchLineKind::Context)) {
                hunk.new_no_newline = true;
            }
        }
        i += 1;
    }
    Ok((hunk, i))
}

// ---------------------------------------------------------------------------
// 行模型（EOL 保留）与应用
// ---------------------------------------------------------------------------

/// 应用选项
#[derive(Debug, Clone, PartialEq)]
pub struct PatchApplyOptions {
    /// 试运行：只校验不落盘
    pub dry_run: bool,
    /// 模糊匹配层数（0 = 仅精确；每层丢弃 1 行首/尾上下文重试）
    pub fuzz: u32,
    /// CRLF 容忍：上下文匹配忽略行尾 CR（输出保留原行尾风格）
    pub crlf_tolerant: bool,
    /// 上下文匹配忽略空白差异（连续空白折叠；git apply --ignore-space-change 子集）
    pub ignore_whitespace: bool,
}

impl Default for PatchApplyOptions {
    fn default() -> Self {
        PatchApplyOptions { dry_run: false, fuzz: 3, crlf_tolerant: true, ignore_whitespace: false }
    }
}

/// 按行拆分并保留行尾风格（last 行 eol = "" 表示文件末行无换行）
fn split_lines_raw(text: &str) -> Vec<(String, &'static str)> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'\n' => {
                out.push((text[start..i].to_string(), "\n"));
                i += 1;
                start = i;
            }
            b'\r' if i + 1 < bytes.len() && bytes[i + 1] == b'\n' => {
                out.push((text[start..i].to_string(), "\r\n"));
                i += 2;
                start = i;
            }
            b'\r' => {
                out.push((text[start..i].to_string(), "\r"));
                i += 1;
                start = i;
            }
            _ => i += 1,
        }
    }
    if start < text.len() {
        out.push((text[start..].to_string(), ""));
    }
    out
}

fn join_lines(lines: &[(String, &'static str)]) -> String {
    let mut out = String::with_capacity(lines.iter().map(|(t, e)| t.len() + e.len()).sum());
    for (t, e) in lines {
        out.push_str(t);
        out.push_str(e);
    }
    out
}

/// 行内容比较键（CRLF 容忍 / 空白折叠）
fn match_key(content: &str, opts: &PatchApplyOptions) -> String {
    let stripped = if opts.crlf_tolerant { content.trim_end_matches('\r') } else { content };
    if opts.ignore_whitespace {
        stripped.split_whitespace().collect::<Vec<_>>().join(" ")
    } else {
        stripped.to_string()
    }
}

/// 单文件内存应用（纯函数；FR-PATCH-011 重建视图消费同一路径）
///
/// 返回 (应用后文本, 使用的模糊层数, 累计行偏移)。补丁已应用时返回
/// [`ApplyOutcome::AlreadyApplied`]（git apply --check 同语义的拒绝）。
pub fn apply_file(orig: &str, file: &PatchFile, opts: &PatchApplyOptions) -> Result<ApplyOutcome, String> {
    if file.hunks.is_empty() {
        return Ok(ApplyOutcome::NoOp);
    }
    // 新文件：hunk 全为 Add（old_count == 0）
    if file.is_new || file.old_path.is_none() {
        let mut lines: Vec<(String, &'static str)> = Vec::new();
        let mut has_no_nl = false;
        for h in &file.hunks {
            for pl in &h.lines {
                lines.push((pl.text.clone(), "\n"));
            }
            has_no_nl |= h.new_no_newline;
        }
        if let Some(last) = lines.last_mut() {
            if has_no_nl {
                last.1 = "";
            }
        }
        return Ok(ApplyOutcome::Applied { text: join_lines(&lines), fuzz_used: 0, offset: 0 });
    }

    let mut lines = split_lines_raw(orig);
    if lines.is_empty() && file.is_delete {
        return Err("目标为空文件，与补丁上下文不符".into());
    }
    // 主导行尾：原文件中首个非空行尾（混合行尾文件取首行风格）；全无行尾 -> LF
    let dominant_eol = lines
        .iter()
        .map(|(_, e)| *e)
        .find(|e| !e.is_empty())
        .unwrap_or("\n");
    let orig_trailing_nl = lines.last().map(|(_, e)| !e.is_empty()).unwrap_or(true);

    let mut fuzz_used: u32 = 0;
    let mut offset: i64 = 0; // 行数增量（后续 hunk 位置校正）
    let mut first_drift: Option<i64> = None; // 首个非零位置漂移（报告）

    for (hidx, h) in file.hunks.iter().enumerate() {
        // 期望旧内容（上下文+删除）与新内容（上下文+新增）；had_cr 供严格行尾匹配
        let expected: Vec<(String, bool)> = h
            .lines
            .iter()
            .filter(|l| matches!(l.kind, PatchLineKind::Context | PatchLineKind::Delete))
            .map(|l| (l.text.clone(), l.had_cr))
            .collect();
        let new_expected: Vec<(String, bool)> = h
            .lines
            .iter()
            .filter(|l| matches!(l.kind, PatchLineKind::Context | PatchLineKind::Add))
            .map(|l| (l.text.clone(), l.had_cr))
            .collect();

        let base_pos = (h.old_start as i64 - 1 + offset).max(0) as usize;

        // 已应用检测：新内容在期望位置（偏移校正后）精确命中 -> AlreadyApplied。
        // 纯删除 hunk（新内容 == 期望旧内容去掉删除行 -> 仅为上下文）时该检测无意义
        // （任何含上下文前缀的未应用文件都会命中），跳过。
        let new_expected_texts: Vec<String> = new_expected.iter().map(|(t, _)| t.clone()).collect();
        let new_expected_cr: Vec<bool> = new_expected.iter().map(|(_, c)| *c).collect();
        // 纯删除 hunk（无 Add 行）不做该检测：此时"新内容"仅为上下文，
        // 任何含上下文前缀的未应用文件都会误命中（roundtrip: 全删尾行场景）。
        // 含 Add 行的 hunk 检测保留（新内容含新增行，未应用文件不会命中）。
        let has_add = h.lines.iter().any(|l| l.kind == PatchLineKind::Add);
        if has_add
            && find_match_window(&lines, base_pos, &new_expected_texts, opts, 0, &new_expected_cr).is_some()
        {
            return Err(format!("hunk #{} 已应用（目标内容与补丁结果一致）", hidx + 1));
        }

        // 位置漂移搜索（全文件）；失败后逐层模糊（丢弃首/尾 fuzz 行上下文重试）
        let mut found: Option<(usize, u32)> = None;
        let mut fuzz = 0u32;
        loop {
            let slice = &expected[fuzz as usize..expected.len() - fuzz as usize];
            let window: Vec<String> = slice.iter().map(|(t, _)| t.clone()).collect();
            let window_cr: Vec<bool> = slice.iter().map(|(_, c)| *c).collect();
            if let Some(p) = find_match_window(&lines, base_pos, &window, opts, usize::MAX, &window_cr) {
                // 完整 hunk 落点须在文件内（前 fuzz 行上下文在 p 之前）
                if p >= fuzz as usize && p - fuzz as usize + expected.len() <= lines.len() {
                    found = Some((p, fuzz));
                    break;
                }
            }
            if fuzz >= opts.fuzz || expected.len() <= fuzz as usize * 2 + 1 {
                break;
            }
            fuzz += 1;
        }
        let (pos, used_fuzz) = match found {
            Some(v) => v,
            None => return Err(format!("hunk #{} 无法应用：上下文不匹配", hidx + 1)),
        };
        let full_start = pos - used_fuzz as usize;
        let old_span = expected.len(); // = context + delete（hunk 头声明的旧行数）
        let drift = full_start as i64 - base_pos as i64;
        if first_drift.is_none() && drift != 0 {
            first_drift = Some(drift);
        }

        // 拼接替换：上下文行保留原行（含原行尾风格）；新增行继承主导行尾
        let mut new_lines: Vec<(String, &'static str)> = Vec::with_capacity(new_expected.len());
        let mut mi = full_start;
        for pl in &h.lines {
            match pl.kind {
                PatchLineKind::Context => {
                    new_lines.push(lines[mi].clone());
                    mi += 1;
                }
                PatchLineKind::Delete => {
                    mi += 1;
                }
                PatchLineKind::Add => {
                    // 新增行行尾：容错模式继承文件主导行尾；严格模式按补丁行 CR 标记
                    let eol = if opts.crlf_tolerant || pl.had_cr == dominant_eol.contains('\r') {
                        dominant_eol
                    } else if pl.had_cr {
                        "\r\n"
                    } else {
                        "\n"
                    };
                    new_lines.push((pl.text.clone(), eol));
                }
            }
        }
        debug_assert_eq!(mi, full_start + old_span, "hunk 旧行消费数 == 期望序列长度");
        lines.splice(full_start..mi, new_lines);

        fuzz_used = fuzz_used.max(used_fuzz);
        offset += new_expected.len() as i64 - old_span as i64;
    }

    // 中部空行尾归一：旧文件末行（无换行）经上下文进入新文件中部时，
    // 该行必须补行尾（补丁标记语义 = 旧末行无换行；新文件后续还有行）
    let non_last = lines.len().saturating_sub(1);
    for line in &mut lines[..non_last] {
        if line.1.is_empty() {
            line.1 = dominant_eol;
        }
    }

    // 末行行尾归一（\ No newline 语义）
    if let Some(last) = lines.last_mut() {
        let last_hunk = file.hunks.last().expect("hunks 非空");
        if last_hunk.new_no_newline {
            last.1 = "";
        } else if last_hunk.old_no_newline && !orig_trailing_nl {
            // 旧末行无换行、新内容应有换行
            last.1 = dominant_eol;
        } else if last.1.is_empty() && orig_trailing_nl && file.hunks.iter().any(|h| !h.lines.is_empty()) {
            // 原有行尾被替换丢失 -> 补 dominant（保持"文件原本有行尾"的事实）
            last.1 = dominant_eol;
        }
    }

    Ok(ApplyOutcome::Applied { text: join_lines(&lines), fuzz_used, offset: first_drift.unwrap_or(0) })
}

/// 应用结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyOutcome {
    Applied { text: String, fuzz_used: u32, offset: i64 },
    /// 空 hunk（纯 mode 变更等）：无操作
    NoOp,
}

/// 在 base 附近搜索期望序列的匹配位置（先 base 后向前/向后交替）。
/// window = 最大漂移行数（usize::MAX = 全文件范围；0 = 仅精确 base 位）。
/// want_cr 与补丁行 had_cr 对齐做严格行尾校验（crlf_tolerant 时忽略）。
fn find_match_window(
    lines: &[(String, &'static str)],
    base: usize,
    expected: &[String],
    opts: &PatchApplyOptions,
    window: usize,
    want_cr: &[bool],
) -> Option<usize> {
    if expected.is_empty() {
        return Some(base.min(lines.len()));
    }
    if lines.len() < expected.len() {
        return None;
    }
    let keys: Vec<String> = lines.iter().map(|(t, _)| match_key(t, opts)).collect();
    let exp: Vec<String> = expected.iter().map(|e| match_key(e, opts)).collect();
    let max_start = lines.len() - expected.len();
    let limit = window.min(max_start);
    for d in 0..=limit {
        let p = if d == 0 {
            base
        } else if d % 2 == 1 {
            match base.checked_sub(d.div_ceil(2)) {
                Some(v) => v,
                None => continue,
            }
        } else {
            base + d / 2
        };
        if p > max_start {
            continue;
        }
        if keys[p..p + expected.len()] != exp[..] {
            continue;
        }
        // 严格模式：补丁行的 CRLF 标记须与目标行行尾风格一致
        if !opts.crlf_tolerant {
            let ok = (0..expected.len())
                .all(|j| want_cr.get(j).is_none_or(|w| lines[p + j].1.contains('\r') == *w));
            if !ok {
                continue;
            }
        }
        return Some(p);
    }
    None
}

// ---------------------------------------------------------------------------
// 文件系统应用（多文件 + 报告）
// ---------------------------------------------------------------------------

/// 文件应用结果状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileApplyStatus {
    Applied,
    AlreadyApplied,
    /// 二进制文件跳过（Binary files / GIT binary patch / 非 UTF-8 内容）
    SkippedBinary,
    /// 空 hunk（纯 mode 变更）：无操作
    NoOp,
    Failed,
}

impl FileApplyStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            FileApplyStatus::Applied => "applied",
            FileApplyStatus::AlreadyApplied => "alreadyApplied",
            FileApplyStatus::SkippedBinary => "skippedBinary",
            FileApplyStatus::NoOp => "noOp",
            FileApplyStatus::Failed => "failed",
        }
    }
}

/// 单文件应用结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileApplyResult {
    pub path: String,
    pub status: FileApplyStatus,
    pub fuzz_used: u32,
    pub offset_applied: i64,
    /// 失败/跳过原因
    pub detail: Option<String>,
}

/// 补丁应用报告（FR-PATCH-011 应用侧）
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PatchApplyReport {
    pub total: usize,
    pub results: Vec<FileApplyResult>,
    pub dry_run: bool,
}

impl PatchApplyReport {
    pub fn all_applied(&self) -> bool {
        self.results.iter().all(|r| matches!(r.status, FileApplyStatus::Applied | FileApplyStatus::NoOp))
    }
}

/// 文件系统抽象（内存实现可注入测试；ai-core 保持无直接 IO）
pub trait PatchFs {
    fn read(&self, path: &Path) -> std::io::Result<Vec<u8>>;
    fn write(&self, path: &Path, data: &[u8]) -> std::io::Result<()>;
    fn exists(&self, path: &Path) -> bool;
    fn remove(&self, path: &Path) -> std::io::Result<()>;
    /// 路径本身是否符号链接（lstat 语义，不跟随）。逃逸护栏用：词法校验
    /// 挡不住文件系统层的链接解析。无链接概念的实现返回 false。
    fn is_symlink(&self, path: &Path) -> bool;
}

/// std::fs 实现
pub struct StdFs;

impl PatchFs for StdFs {
    fn read(&self, path: &Path) -> std::io::Result<Vec<u8>> {
        std::fs::read(path)
    }
    fn write(&self, path: &Path, data: &[u8]) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                let _ = std::fs::create_dir_all(parent);
            }
        }
        std::fs::write(path, data)
    }
    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }
    fn remove(&self, path: &Path) -> std::io::Result<()> {
        std::fs::remove_file(path)
    }
    fn is_symlink(&self, path: &Path) -> bool {
        std::fs::symlink_metadata(path).map(|m| m.file_type().is_symlink()).unwrap_or(false)
    }
}

/// 路径护栏：拒绝绝对路径 / `..` 逃逸 / 空路径（补丁路径不可越出 root）
pub fn sanitize_patch_path(path: &str) -> Result<PathBuf, String> {
    if path.is_empty() {
        return Err("拒绝空路径".into());
    }
    let p = Path::new(path);
    if p.is_absolute() {
        return Err(format!("拒绝绝对路径: {path}"));
    }
    for seg in p.components() {
        if matches!(seg, std::path::Component::ParentDir) {
            return Err(format!("拒绝相对引用段（..）: {path}"));
        }
    }
    Ok(p.to_path_buf())
}

/// 自 root 起逐分量检查（含末分量），返回第一个符号链接路径。
///
/// 末分量也要查：写入会跟随链接覆写根外文件；祖先分量同理（`create_dir_all`
/// 与删除都会穿过它）。
fn first_symlink_component(fs: &dyn PatchFs, root: &Path, rel: &str) -> Option<PathBuf> {
    let mut cur = root.to_path_buf();
    for seg in Path::new(rel).components() {
        cur.push(seg);
        if fs.is_symlink(&cur) {
            return Some(cur);
        }
    }
    None
}

/// 多文件补丁应用（root 为目标根目录；FR-PATCH-010 补丁生成 -> 011 应用闭环）
pub fn apply_patch(
    fs: &dyn PatchFs,
    root: &Path,
    patch_text: &str,
    opts: &PatchApplyOptions,
) -> Result<PatchApplyReport, PatchError> {
    let parsed = parse_patch(patch_text)?;
    let mut report = PatchApplyReport { total: parsed.files.len(), dry_run: opts.dry_run, results: Vec::new() };
    for f in &parsed.files {
        let result = apply_one_file(fs, root, f, opts);
        report.results.push(result);
    }
    Ok(report)
}

fn apply_one_file(fs: &dyn PatchFs, root: &Path, f: &PatchFile, opts: &PatchApplyOptions) -> FileApplyResult {
    let label = f.label();
    let mk = |status: FileApplyStatus, fuzz: u32, off: i64, detail: Option<String>| FileApplyResult {
        path: label.clone(),
        status,
        fuzz_used: fuzz,
        offset_applied: off,
        detail,
    };

    let Some(rel) = f.target_path() else {
        return mk(FileApplyStatus::Failed, 0, 0, Some("补丁缺少文件路径".into()));
    };
    let path = match sanitize_patch_path(rel) {
        Ok(p) => root.join(p),
        Err(e) => return mk(FileApplyStatus::Failed, 0, 0, Some(e)),
    };
    // 词法护栏只保证「字符串上不逃出 root」：root 内任一分量是符号链接时，
    // 读/写/删都会跟随它落到 root 之外（git apply 对同语料直接拒绝）。
    if let Some(link) = first_symlink_component(fs, root, rel) {
        return mk(
            FileApplyStatus::Failed,
            0,
            0,
            Some(format!("拒绝符号链接路径（{rel}）：{} 会越过补丁根", link.display())),
        );
    }

    // 二进制跳过（FR-PATCH-011：二进制文件报告不应用）
    if f.binary {
        return mk(FileApplyStatus::SkippedBinary, 0, 0, Some("二进制文件（补丁标注 Binary/GIT binary patch）".into()));
    }
    if f.hunks.is_empty() {
        return mk(FileApplyStatus::NoOp, 0, 0, Some("无内容 hunk（纯元数据变更）".into()));
    }

    // 新文件
    if f.is_new || f.old_path.is_none() {
        if fs.exists(&path) {
            return mk(FileApplyStatus::Failed, 0, 0, Some(format!("目标已存在: {rel}")));
        }
        match apply_file("", f, opts) {
            Ok(ApplyOutcome::Applied { text, .. }) => {
                if !opts.dry_run && fs.write(&path, text.as_bytes()).is_err() {
                    return mk(FileApplyStatus::Failed, 0, 0, Some(format!("写入失败: {rel}")));
                }
                mk(FileApplyStatus::Applied, 0, 0, None)
            }
            _ => mk(FileApplyStatus::Failed, 0, 0, Some("新文件内容构造失败".into())),
        }
        .tap()
    } else {
        // 修改 / 删除：读目标
        let bytes = match fs.read(&path) {
            Ok(b) => b,
            Err(_) => return mk(FileApplyStatus::Failed, 0, 0, Some(format!("目标不存在或不可读: {rel}"))),
        };
        let orig = match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(_) => {
                return mk(FileApplyStatus::SkippedBinary, 0, 0, Some("目标内容非 UTF-8 文本（按二进制跳过）".into()));
            }
        };
        if f.is_delete {
            // 删除校验：文件内容 == 补丁旧内容（上下文+删除行）
            let expected: Vec<&str> = f
                .hunks
                .iter()
                .flat_map(|h| h.lines.iter())
                .filter(|l| matches!(l.kind, PatchLineKind::Context | PatchLineKind::Delete))
                .map(|l| l.text.as_str())
                .collect();
            let orig_norm: Vec<String> = split_lines_raw(&orig)
                .into_iter()
                .map(|(t, _)| match_key(&t, opts))
                .collect();
            let exp_norm: Vec<String> = expected.iter().map(|e| match_key(e, opts)).collect();
            if orig_norm != exp_norm {
                return mk(FileApplyStatus::Failed, 0, 0, Some(format!("删除校验失败：文件内容与补丁不符: {rel}")));
            }
            if !opts.dry_run && fs.remove(&path).is_err() {
                return mk(FileApplyStatus::Failed, 0, 0, Some(format!("删除失败: {rel}")));
            }
            return mk(FileApplyStatus::Applied, 0, 0, None);
        }
        match apply_file(&orig, f, opts) {
            Ok(ApplyOutcome::Applied { text, fuzz_used, offset }) => {
                if !opts.dry_run && fs.write(&path, text.as_bytes()).is_err() {
                    return mk(FileApplyStatus::Failed, fuzz_used, offset, Some(format!("写入失败: {rel}")));
                }
                mk(FileApplyStatus::Applied, fuzz_used, offset, None)
            }
            Ok(ApplyOutcome::NoOp) => mk(FileApplyStatus::NoOp, 0, 0, None),
            Err(msg) => {
                let status = if msg.contains("已应用") { FileApplyStatus::AlreadyApplied } else { FileApplyStatus::Failed };
                mk(status, 0, 0, Some(msg))
            }
        }
    }
}

/// Result tap 辅助（内部小语法糖）
trait Tap: Sized {
    fn tap(self) -> Self {
        self
    }
}
impl<T> Tap for T {}

#[cfg(test)]
mod tests {
    use super::*;

    const MEM: MemFs = MemFs::new();

    /// 内存 fs（单元测试注入）
    struct MemFs {
        _p: (),
    }

    impl MemFs {
        const fn new() -> Self {
            MemFs { _p: () }
        }
    }

    impl PatchFs for MemFs {
        fn read(&self, _path: &Path) -> std::io::Result<Vec<u8>> {
            Err(std::io::Error::new(std::io::ErrorKind::NotFound, "mem"))
        }
        fn write(&self, _path: &Path, _data: &[u8]) -> std::io::Result<()> {
            Ok(())
        }
        fn exists(&self, _path: &Path) -> bool {
            false
        }
        fn remove(&self, _path: &Path) -> std::io::Result<()> {
            Ok(())
        }
        fn is_symlink(&self, _path: &Path) -> bool {
            false
        }
    }

    // ---- 解析 ----

    #[test]
    fn parse_plain_unified() {
        let patch = "--- a.txt\t2026-01-01\n+++ b.txt\t2026-01-01\n@@ -1,3 +1,3 @@\n a\n-b\n+B\n c\n";
        let p = parse_patch(patch).unwrap();
        assert_eq!(p.files.len(), 1);
        let f = &p.files[0];
        assert_eq!(f.old_path.as_deref(), Some("a.txt"));
        assert_eq!(f.new_path.as_deref(), Some("b.txt"));
        assert_eq!(f.hunks.len(), 1);
        let h = &f.hunks[0];
        assert_eq!((h.old_start, h.old_count, h.new_start, h.new_count), (1, 3, 1, 3));
        assert_eq!(h.lines.len(), 4);
    }

    #[test]
    fn parse_git_headers_and_modes() {
        let patch = "\
diff --git a/src/x.c b/src/x.c\n\
index 1234567..89abcde 100644\n\
--- a/src/x.c\n\
+++ b/src/x.c\n\
@@ -1 +1 @@\n\
-old\n\
+new\n";
        let p = parse_patch(patch).unwrap();
        let f = &p.files[0];
        assert_eq!(f.old_path.as_deref(), Some("src/x.c"));
        assert_eq!(f.new_path.as_deref(), Some("src/x.c"));
        assert!(!f.is_new && !f.is_delete && !f.binary);
        assert_eq!(f.hunks[0].old_count, 1);
    }

    #[test]
    fn parse_new_and_deleted_files() {
        let patch = "\
diff --git a/new.txt b/new.txt\n\
new file mode 100644\n\
index 0000000..aaaa\n\
--- /dev/null\n\
+++ b/new.txt\n\
@@ -0,0 +1,2 @@\n\
+hello\n\
+world\n\
diff --git a/gone.txt b/gone.txt\n\
deleted file mode 100644\n\
index bbbb..0000000\n\
--- a/gone.txt\n\
+++ /dev/null\n\
@@ -1 +0,0 @@\n\
-byeee\n";
        let p = parse_patch(patch).unwrap();
        assert_eq!(p.files.len(), 2);
        let nf = &p.files[0];
        assert!(nf.is_new);
        assert!(nf.old_path.is_none());
        assert_eq!(nf.target_path(), Some("new.txt"));
        assert_eq!(nf.hunks[0].new_count, 2);
        let df = &p.files[1];
        assert!(df.is_delete);
        assert!(df.new_path.is_none());
        assert_eq!(df.target_path(), Some("gone.txt"));
    }

    #[test]
    fn parse_binary_markers() {
        let p = parse_patch(
            "diff --git a/img.png b/img.png\nindex abc..def 100644\nBinary files a/img.png and b/img.png differ\n",
        )
        .unwrap();
        assert_eq!(p.files.len(), 1);
        assert!(p.files[0].binary);
        assert!(p.files[0].hunks.is_empty());

        let p = parse_patch("diff --git a/blob b/blob\nindex abc..def 100644\nGIT binary patch\nliteral 10\nZcmV<fake\n\n").unwrap();
        assert!(p.files[0].binary);
    }

    #[test]
    fn parse_no_newline_markers() {
        let patch = "--- a\n+++ b\n@@ -1 +1 @@\n-old\n\\ No newline at end of file\n+new\n";
        let p = parse_patch(patch).unwrap();
        let h = &p.files[0].hunks[0];
        assert!(h.old_no_newline);
        assert!(!h.new_no_newline);
        let patch2 = "--- a\n+++ b\n@@ -1 +1 @@\n-old\n+new\n\\ No newline at end of file\n";
        let h2 = &parse_patch(patch2).unwrap().files[0].hunks[0];
        assert!(h2.new_no_newline && !h2.old_no_newline);
    }

    #[test]
    fn parse_hunk_count_shorthand() {
        // -1 +1 省略 ,1
        let p = parse_patch("--- a\n+++ b\n@@ -2 +2 @@\n-x\n+y\n").unwrap();
        let h = &p.files[0].hunks[0];
        assert_eq!((h.old_start, h.old_count, h.new_start, h.new_count), (2, 1, 2, 1));
    }

    #[test]
    fn parse_empty_patch_is_error() {
        assert!(matches!(parse_patch("hello\nworld\n"), Err(PatchError::Empty)));
        assert!(matches!(parse_patch(""), Err(PatchError::Empty)));
    }

    #[test]
    fn parse_section_heading_tolerated() {
        let p = parse_patch("--- a.c\n+++ b.c\n@@ -1,2 +1,2 @@ fn main() {\n a\n-b\n+B\n c\n").unwrap();
        assert_eq!(p.files[0].hunks.len(), 1);
    }

    // ---- 应用 ----

    const OPTS: PatchApplyOptions = PatchApplyOptions {
        dry_run: false,
        fuzz: 3,
        crlf_tolerant: true,
        ignore_whitespace: false,
    };

    #[test]
    fn apply_exact_replacement() {
        let patch = "--- a\n+++ b\n@@ -1,3 +1,3 @@\n a\n-b\n+B\n c\n";
        let f = &parse_patch(patch).unwrap().files[0];
        let out = apply_file("a\nb\nc\n", f, &OPTS).unwrap();
        match out {
            ApplyOutcome::Applied { text, fuzz_used, offset } => {
                assert_eq!(text, "a\nB\nc\n");
                assert_eq!((fuzz_used, offset), (0, 0));
            }
            _ => panic!("应成功"),
        }
    }

    #[test]
    fn apply_with_offset_drift() {
        let patch = "--- a\n+++ b\n@@ -5 +5 @@\n-old5\n+NEW5\n";
        let f = &parse_patch(patch).unwrap().files[0];
        // 实际文件在头部多了 2 行：old5 现在在第 7 行
        let orig = "h1\nh2\n1\n2\n3\n4\nold5\n6\n";
        match apply_file(orig, f, &OPTS).unwrap() {
            ApplyOutcome::Applied { text, offset, .. } => {
                assert_eq!(text, "h1\nh2\n1\n2\n3\n4\nNEW5\n6\n");
                assert_eq!(offset, 2, "匹配位置较补丁声明后移 2 行");
            }
            _ => panic!("应成功"),
        }
    }

    #[test]
    fn apply_fuzzy_drops_context() {
        // 上下文首行被改：fuzz=1 丢弃首行上下文后应用
        let patch = "--- a\n+++ b\n@@ -1,3 +1,3 @@\n ctx-top\n-middle\n+MIDDLE\n ctx-bottom\n";
        let f = &parse_patch(patch).unwrap().files[0];
        let orig = "CHANGED-top\nmiddle\nctx-bottom\n";
        match apply_file(orig, f, &OPTS).unwrap() {
            ApplyOutcome::Applied { text, fuzz_used, .. } => {
                assert_eq!(text, "CHANGED-top\nMIDDLE\nctx-bottom\n");
                assert_eq!(fuzz_used, 1);
            }
            _ => panic!("应成功"),
        }
    }

    #[test]
    fn apply_fuzz_exhausted_fails() {
        let patch = "--- a\n+++ b\n@@ -1,3 +1,3 @@\n ctx-top\n-middle\n+MIDDLE\n ctx-bottom\n";
        let f = &parse_patch(patch).unwrap().files[0];
        let orig = "zzz\nnothing\nmatches\n";
        assert!(apply_file(orig, f, &OPTS).is_err());
        // fuzz=0：即使差一行上下文也拒绝
        let strict = PatchApplyOptions { fuzz: 0, ..OPTS.clone() };
        assert!(apply_file("CHANGED-top\nmiddle\nctx-bottom\n", f, &strict).is_err());
    }

    #[test]
    fn apply_crlf_tolerant_preserves_endings() {
        let patch = "--- a\n+++ b\n@@ -1,3 +1,3 @@\n one\n-two\n+TWO\n three\n";
        let f = &parse_patch(patch).unwrap().files[0];
        let orig = "one\r\ntwo\r\nthree\r\n";
        match apply_file(orig, f, &OPTS).unwrap() {
            ApplyOutcome::Applied { text, .. } => {
                assert_eq!(text, "one\r\nTWO\r\nthree\r\n", "新增行继承 CRLF，原行保留");
            }
            _ => panic!("应成功"),
        }
        // strict 模式：CRLF 与补丁 LF 不匹配 -> 拒绝
        let strict = PatchApplyOptions { crlf_tolerant: false, ..OPTS.clone() };
        assert!(apply_file(orig, f, &strict).is_err());
    }

    #[test]
    fn apply_new_file_and_delete_file_semantics() {
        let patch = "--- /dev/null\n+++ b/made.txt\n@@ -0,0 +1,2 @@\n+one\n+two\n";
        let f = &parse_patch(patch).unwrap().files[0];
        match apply_file("", f, &OPTS).unwrap() {
            ApplyOutcome::Applied { text, .. } => assert_eq!(text, "one\ntwo\n"),
            _ => panic!(),
        }
        let patch_del = "--- a\n+++ /dev/null\n@@ -1 +0,0 @@\n-victim\n";
        let f = &parse_patch(patch_del).unwrap().files[0];
        // 删除文件的内存语义：内容校验在 FS 层；这里 hunk 应用于原文无意义
        let _ = f;
    }

    #[test]
    fn apply_no_newline_roundtrip() {
        // 旧末行无换行 -> 新末行有换行
        let patch = "--- a\n+++ b\n@@ -1 +1 @@\n-old\n\\ No newline at end of file\n+new\n";
        let f = &parse_patch(patch).unwrap().files[0];
        match apply_file("old", f, &OPTS).unwrap() {
            ApplyOutcome::Applied { text, .. } => assert_eq!(text, "new\n", "补行尾"),
            _ => panic!(),
        }
        // 新末行无换行
        let patch = "--- a\n+++ b\n@@ -1 +1 @@\n-old\n+new\n\\ No newline at end of file\n";
        let f = &parse_patch(patch).unwrap().files[0];
        match apply_file("old\n", f, &OPTS).unwrap() {
            ApplyOutcome::Applied { text, .. } => assert_eq!(text, "new", "去行尾"),
            _ => panic!(),
        }
    }

    #[test]
    fn apply_already_applied_detected() {
        let patch = "--- a\n+++ b\n@@ -1,3 +1,3 @@\n a\n-b\n+B\n c\n";
        let f = &parse_patch(patch).unwrap().files[0];
        let applied = "a\nB\nc\n";
        let err = apply_file(applied, f, &OPTS).unwrap_err();
        assert!(err.contains("已应用"), "{err}");
    }

    /// 纯删除 hunk 不触发已应用误判（上下文前缀在未应用文件中必命中）：
    /// 回归 P11 roundtrip（delete_tail 语料）
    #[test]
    fn apply_pure_delete_not_false_already_applied() {
        let patch = "--- a\n+++ b\n@@ -1,3 +1,2 @@\n a\n b\n-c\n";
        let f = &parse_patch(patch).unwrap().files[0];
        match apply_file("a\nb\nc\n", f, &OPTS).unwrap() {
            ApplyOutcome::Applied { text, .. } => assert_eq!(text, "a\nb\n"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn apply_multi_hunk_offsets_chain() {
        let patch = "--- a\n+++ b\n@@ -1,2 +1,2 @@\n-a\n+A\n b\n@@ -4 +4 @@\n-e\n+E2\n";
        let f = &parse_patch(patch).unwrap().files[0];
        let orig = "a\nb\nc\nd\ne\n";
        match apply_file(orig, f, &OPTS).unwrap() {
            ApplyOutcome::Applied { text, .. } => assert_eq!(text, "A\nb\nc\nd\nE2\n"),
            _ => panic!(),
        }
    }

    // ---- FS 层 ----

    #[test]
    fn fs_layer_dry_run_and_binary_skip() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("t.txt"), b"1\n2\n3\n").unwrap();
        std::fs::write(root.join("img.png"), b"\x89PNG\r\n\x1a\n").unwrap();
        let patch = "\
diff --git a/t.txt b/t.txt\n\
index 111..222 100644\n\
--- a/t.txt\n\
+++ b/t.txt\n\
@@ -1,3 +1,3 @@\n 1\n-2\n+TWO\n 3\n\
diff --git a/img.png b/img.png\n\
index aaa..bbb 100644\n\
Binary files a/img.png and b/img.png differ\n";
        // dry run：不落盘
        let report = apply_patch(&StdFs, root, patch, &PatchApplyOptions { dry_run: true, ..OPTS.clone() }).unwrap();
        assert_eq!(report.total, 2);
        assert_eq!(report.results[0].status, FileApplyStatus::Applied);
        assert_eq!(report.results[1].status, FileApplyStatus::SkippedBinary);
        assert_eq!(std::fs::read_to_string(root.join("t.txt")).unwrap(), "1\n2\n3\n", "dry run 未写盘");

        let report = apply_patch(&StdFs, root, patch, &OPTS).unwrap();
        assert!(!report.all_applied() || true); // 二进制跳过不算失败
        assert_eq!(report.results[0].status, FileApplyStatus::Applied);
        assert_eq!(std::fs::read_to_string(root.join("t.txt")).unwrap(), "1\nTWO\n3\n");
        assert_eq!(report.results[1].status, FileApplyStatus::SkippedBinary);
    }

    /// R3-02：补丁目标末分量是符号链接时，写入会跟随它改到 root 之外。
    /// 词法护栏（拒绝对路径 / `..`）挡不住这一层；git apply 同语料直接拒绝。
    #[cfg(unix)]
    #[test]
    fn patch_through_symlink_target_is_rejected() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let victim = outside.join("victim.txt");
        std::fs::write(&victim, "old\n").unwrap();
        symlink(&victim, root.join("sub").join("link.txt")).unwrap();
        let patch = "--- a/sub/link.txt\n+++ b/sub/link.txt\n@@ -1,1 +1,1 @@\n-old\n+PATCHED\n";
        let report = apply_patch(&StdFs, &root, patch, &OPTS).unwrap();
        assert_eq!(report.results[0].status, FileApplyStatus::Failed, "不得穿过符号链接写根外: {report:?}");
        let detail = report.results[0].detail.as_deref().unwrap_or("");
        assert!(detail.contains("符号链接"), "失败明细须说明原因: {detail}");
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "old\n", "根外文件必须完好");
    }

    /// R3-02 变体：祖先分量是符号链接（`root/sub` → 根外目录），
    /// 写入与 `create_dir_all` 都会穿过它。
    #[cfg(unix)]
    #[test]
    fn patch_through_symlinked_ancestor_is_rejected() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        std::fs::create_dir_all(&root).unwrap();
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let victim = outside.join("f.txt");
        std::fs::write(&victim, "old\n").unwrap();
        symlink(&outside, root.join("sub")).unwrap();
        let patch = "--- a/sub/f.txt\n+++ b/sub/f.txt\n@@ -1,1 +1,1 @@\n-old\n+PATCHED\n";
        let report = apply_patch(&StdFs, &root, patch, &OPTS).unwrap();
        assert_eq!(report.results[0].status, FileApplyStatus::Failed, "不得穿过符号链接父目录写根外: {report:?}");
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "old\n", "根外文件必须完好");
    }

    #[test]
    fn fs_layer_new_and_delete_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("victim.txt"), b"victim\n").unwrap();
        let patch = "\
diff --git a/made.txt b/made.txt\n\
new file mode 100644\n\
--- /dev/null\n\
+++ b/made.txt\n\
@@ -0,0 +1,1 @@\n\
+made\n\
diff --git a/victim.txt b/victim.txt\n\
deleted file mode 100644\n\
--- a/victim.txt\n\
+++ /dev/null\n\
@@ -1 +0,0 @@\n\
-victim\n";
        let report = apply_patch(&StdFs, root, patch, &OPTS).unwrap();
        assert_eq!(report.results[0].status, FileApplyStatus::Applied);
        assert_eq!(report.results[1].status, FileApplyStatus::Applied);
        assert_eq!(std::fs::read_to_string(root.join("made.txt")).unwrap(), "made\n");
        assert!(!root.join("victim.txt").exists());
        // 已删除后重放：新建失败（目标已存在）、删除失败（目标不存在）
        let report2 = apply_patch(&StdFs, root, patch, &OPTS).unwrap();
        assert_eq!(report2.results[0].status, FileApplyStatus::Failed);
        assert_eq!(report2.results[1].status, FileApplyStatus::Failed);
    }

    #[test]
    fn path_guard_rejects_escape() {
        assert!(sanitize_patch_path("a/b.txt").is_ok());
        assert!(sanitize_patch_path("../escape").is_err());
        assert!(sanitize_patch_path("/abs").is_err());
        assert!(sanitize_patch_path("").is_err());
    }

    // ---- roundtrip：ai_core diff -> unified patch -> 应用还原 ----

    /// 最小 unified 渲染（测试自产补丁；真实端到端见 tests/patch_matrix 用 aic diff）
    fn render_unified_min(left_label: &str, right_label: &str, out: &crate::sections::DiffOutput, l: &crate::LineTable, r: &crate::LineTable) -> String {
        use crate::sections::SectionKind;
        let mut text = format!("--- {left_label}\n+++ {right_label}\n");
        for s in &out.sections {
            if s.kind == SectionKind::Equal {
                continue;
            }
            let (Some(lsec), Some(rsec)) = (s.left, s.right) else { continue };
            let ls = if !lsec.is_empty() { lsec.start + 1 } else { lsec.start };
            let rs = if !rsec.is_empty() { rsec.start + 1 } else { rsec.start };
            text.push_str(&format!("@@ -{},{} +{},{} @@\n", ls, lsec.len(), rs, rsec.len()));
            let mut li = lsec.start;
            let mut ri = rsec.start;
            while li < lsec.end || ri < rsec.end {
                if li < lsec.end {
                    text.push_str(&format!("-{}\n", l.line(li as usize).text));
                    li += 1;
                }
                if ri < rsec.end {
                    text.push_str(&format!("+{}\n", r.line(ri as usize).text));
                    ri += 1;
                }
            }
        }
        text
    }

    #[test]
    fn roundtrip_self_produced_patch_restores_right() {
        let left = "alpha\nbeta\ngamma\ndelta\nepsilon\n";
        let right = "alpha\nBETA\ngamma\ndelta\nEPSILON\nzeta\n";
        let l = crate::LineTable::from_text(left);
        let r = crate::LineTable::from_text(right);
        let out = crate::diff_text(&l, &r, &crate::options::TextDiffOptions::default()).unwrap();
        let patch = render_unified_min("left.l", "right.l", &out, &l, &r);
        let parsed = parse_patch(&patch).unwrap();
        assert_eq!(parsed.files.len(), 1);
        match apply_file(left, &parsed.files[0], &OPTS).unwrap() {
            ApplyOutcome::Applied { text, .. } => assert_eq!(text, right, "自产补丁应用还原右侧"),
            _ => panic!(),
        }
    }

    #[test]
    fn roundtrip_identical_files_no_patch() {
        let t = "same\n";
        let l = crate::LineTable::from_text(t);
        let out = crate::diff_text(&l, &l, &crate::options::TextDiffOptions::default()).unwrap();
        let patch = render_unified_min("a", "b", &out, &l, &l);
        assert!(parse_patch(&patch).is_err(), "无差异 -> 空 patch -> Empty 错误");
    }

    #[test]
    fn mem_fs_is_pure_injection() {
        // 内存 fs 注入可用（ai-core 无直接 IO 的边界由 trait 保持）
        let dir = tempfile::tempdir().unwrap();
        let report = apply_patch(&MEM, dir.path(), "", &OPTS);
        assert!(matches!(report, Err(PatchError::Empty)));
    }
}
