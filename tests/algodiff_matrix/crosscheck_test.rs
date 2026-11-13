//! 算法对拍 golden 矩阵（docs/04 P1 交付物 ⑥；docs/03 §8.2 测试策略 3）。
//!
//! 与两套独立参考实现——Python difflib（SequenceMatcher，autojunk=false）与
//! git diff --no-index -U0——对拍。fixtures 由 `generate_fixtures.py` 预生成
//! （生成时校验两套参考实现互相一致，即语料对齐无歧义）。
//!
//! - `exact = true` 用例：ai-core 四种算法（Standard/Myers/Patience/Histogram）
//!   的 hunks 必须与参考实现完全一致；
//! - `exact = false` 用例（移动块/高重复行，对齐合法但不唯一）：只做性质校验
//!   —— 重放还原右侧 + 无假等值 + 无缝覆盖 + 变更行数与参考一致（编辑量守恒：
//!   删+插行数相等，因两侧 LCS 相同）。
//!
//! 重新固化：`python3 tests/algodiff_matrix/generate_fixtures.py`。

use ai_compare_tests as _; // 保持测试 crate 链接（未直接使用其设施）
use ai_core::options::{DiffConfig, LineAlignAlgo, ManualAlignOptions, TextDiffOptions};
use ai_core::sections::SectionKind;
use ai_core::{diff_config, LineTable};

/// hunks：[left_start, left_end, right_start, right_end]（0 起半开）
type Hunk = [u32; 4];

fn algo_cfg(algo: LineAlignAlgo) -> DiffConfig {
    DiffConfig {
        base: TextDiffOptions { algo, ..TextDiffOptions::default() },
        manual: ManualAlignOptions::default(),
        ..DiffConfig::default()
    }
}

/// sections -> hunks（非 Equal section；游标推进与 build_sections 一致）
fn sections_to_hunks(sections: &[ai_core::Section]) -> Vec<Hunk> {
    let mut out = Vec::new();
    let (mut lc, mut rc) = (0u32, 0u32);
    for s in sections {
        match s.kind {
            SectionKind::Equal => {
                lc = s.left.expect("Equal 必有左区间").end;
                rc = s.right.expect("Equal 必有右区间").end;
            }
            SectionKind::Change => {
                let l = s.left.expect("Change 必有左区间");
                let r = s.right.expect("Change 必有右区间");
                out.push([l.start, l.end, r.start, r.end]);
                lc = l.end;
                rc = r.end;
            }
            SectionKind::Delete => {
                let l = s.left.expect("Delete 必有左区间");
                out.push([l.start, l.end, rc, rc]);
                lc = l.end;
            }
            SectionKind::Insert => {
                let r = s.right.expect("Insert 必有右区间");
                out.push([lc, lc, r.start, r.end]);
                rc = r.end;
            }
        }
    }
    out
}

/// 性质校验：无缝覆盖 + 无假等值 + Equal 配对行数一致。
///
/// 注意：默认骨架忽略行尾风格（FR-TXT-010 / docs/03 §6.1），Equal 段两侧
/// 行尾可能不同（CRLF 混合语料），因此逐行比较**行内容**而非整文件字节。
fn assert_properties(left: &LineTable, right: &LineTable, sections: &[ai_core::Section], name: &str) {
    let (mut lc, mut rc) = (0u32, 0u32);
    for s in sections {
        if let Some(l) = s.left {
            assert_eq!(l.start, lc, "{name}: 左区间必须连续");
            lc = l.end;
        }
        if let Some(r) = s.right {
            assert_eq!(r.start, rc, "{name}: 右区间必须连续");
            rc = r.end;
        }
        if s.kind == SectionKind::Equal {
            let l = s.left.expect("Equal 必有左区间");
            let r = s.right.expect("Equal 必有右区间");
            assert_eq!(l.len(), r.len(), "{name}: Equal 等长");
            for k in 0..l.len() {
                assert_eq!(
                    left.line((l.start + k) as usize).text,
                    right.line((r.start + k) as usize).text,
                    "{name}: Equal 段第 {k} 行两侧必须一致（无假等值）"
                );
            }
        }
    }
    assert_eq!(lc, left.len() as u32, "{name}: 左侧无缝覆盖");
    assert_eq!(rc, right.len() as u32, "{name}: 右侧无缝覆盖");
}

/// 编辑量守恒：删+插行数与参考 hunks 一致（LCS 相同则编辑量相同）
fn changed_lines(hunks: &[Hunk]) -> u32 {
    hunks.iter().map(|h| (h[1] - h[0]) + (h[3] - h[2])).sum()
}

fn load_fixtures() -> serde_json::Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/algodiff_matrix/fixtures.json");
    let raw = std::fs::read_to_string(path).expect("fixtures.json 存在（先运行 generate_fixtures.py）");
    serde_json::from_str(&raw).expect("fixtures.json 合法")
}

fn hunks_of(case: &serde_json::Value, key: &str) -> Vec<Hunk> {
    case[key]
        .as_array()
        .expect("hunks 数组")
        .iter()
        .map(|h| {
            let a = h.as_array().expect("hunk 数组");
            [
                a[0].as_u64().unwrap() as u32,
                a[1].as_u64().unwrap() as u32,
                a[2].as_u64().unwrap() as u32,
                a[3].as_u64().unwrap() as u32,
            ]
        })
        .collect()
}

const ALGOS: [(&str, LineAlignAlgo); 4] = [
    ("standard", LineAlignAlgo::Standard),
    ("myers", LineAlignAlgo::Myers),
    ("patience", LineAlignAlgo::Patience),
    ("histogram", LineAlignAlgo::Histogram),
];

#[test]
fn algodiff_matrix_matches_difflib_and_git() {
    let fixtures = load_fixtures();
    let cases = fixtures["cases"].as_array().expect("cases 数组");
    assert!(cases.len() >= 20, "语料覆盖应 >= 20 用例");

    for case in cases {
        let name = case["name"].as_str().expect("用例名");
        let left = LineTable::from_text(case["left_raw"].as_str().expect("left_raw"));
        let right = LineTable::from_text(case["right_raw"].as_str().expect("right_raw"));
        let exact = case["exact"].as_bool().expect("exact 标志");
        let difflib = hunks_of(case, "difflib_hunks");
        let git = hunks_of(case, "git_hunks");

        for (algo_name, algo) in ALGOS {
            let cfg = algo_cfg(algo);
            let out = diff_config(&left, &right, &cfg)
                .unwrap_or_else(|e| panic!("{name}/{algo_name}: diff 失败: {e}"));
            let mine = sections_to_hunks(&out.output.sections);

            if exact {
                assert_eq!(
                    mine, difflib,
                    "{name}/{algo_name}: hunks 与 difflib 不一致"
                );
                assert_eq!(mine, git, "{name}/{algo_name}: hunks 与 git 不一致");
            } else {
                assert_properties(&left, &right, &out.output.sections, &format!("{name}/{algo_name}"));
                // 编辑量守恒：两种合法对齐的 删+插 行数必须相同（LCS 不变）
                assert_eq!(
                    changed_lines(&mine),
                    changed_lines(&difflib),
                    "{name}/{algo_name}: 编辑量与参考实现不一致"
                );
            }
            // 所有用例（含 exact）都做性质校验
            assert_properties(&left, &right, &out.output.sections, &format!("{name}/{algo_name}"));
        }
    }
}

#[test]
fn importance_toggle_keeps_skeleton() {
    // FR-TXT-010：切换重要性规则不改变行对齐骨架（只重判）
    let left = LineTable::from_text("Alpha  \nkeep\n");
    let right = LineTable::from_text("alpha\nkeep\n");
    let imp = ai_core::options::ImportanceOptions {
        case_unimportant: true,
        trailing_ws_unimportant: true,
        ..ai_core::options::ImportanceOptions::default()
    };
    let strict_cfg = DiffConfig::default();
    let relaxed_cfg = DiffConfig { importance: imp, ..DiffConfig::default() };

    let strict = diff_config(&left, &right, &strict_cfg).unwrap();
    let relaxed = diff_config(&left, &right, &relaxed_cfg).unwrap();
    // 骨架（kind + 区间）完全一致；重要性不同
    /// 骨架指纹：kind + 两侧区间
    type Skeleton = Vec<(SectionKind, Option<[u32; 2]>, Option<[u32; 2]>)>;
    fn skel(o: &ai_core::KeyedDiffOutput) -> Skeleton {
        o.output
            .sections
            .iter()
            .map(|s| (s.kind, s.left.map(|r| [r.start, r.end]), s.right.map(|r| [r.start, r.end])))
            .collect()
    }
    assert_eq!(skel(&strict), skel(&relaxed), "重要性开关不得改变骨架");
    assert!(relaxed.unimportant_sections >= 1, "大小写+行尾空白差异应为不重要");
    assert_eq!(strict.unimportant_sections, 0);
}
