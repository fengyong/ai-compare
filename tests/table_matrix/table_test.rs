//! 表格对拍矩阵（P9 / docs/04 §P9 验收要点：CSV 边角语料对拍）。
//!
//! 双层对拍：
//! 1. **解析层**：与 Python csv 模块（RFC4180 参考实现）逐单元格一致 ——
//!    fixtures.json 由 `generate_fixtures.py` 生成（18 条语料：引号嵌套/
//!    转义/换行内嵌/BOM/CRLF/空列/松散行/Unicode/键重复/列序打乱/TSV/
//!    未闭合引号）。约定差异（空白行、BOM）在生成器侧归一（见脚本 doc）。
//! 2. **diff 语义层**：行类别 / 单元格状态 golden（手写断言，对齐 BC
//!    FR-TBL-003/004 语义：键列匹配、列名对齐、偏斜补配、重要性红蓝点）。
//!
//! 重新固化 fixtures：`python3 tests/table_matrix/generate_fixtures.py`。

use ai_compare_tests as _;
use ai_core::table::{
    compare_cell, diff_table_data, diff_tables, parse_table, CellDiffStatus, ColumnAlign,
    ColumnType, RowAlign, TableDiffOptions, TableImportance, TableParseOptions,
};
use ai_core::options::WhitespaceMode;

const FIXTURES: &str = include_str!("../table_matrix/fixtures.json");

fn fixtures() -> Vec<serde_json::Value> {
    serde_json::from_str(FIXTURES).expect("fixtures.json 合法")
}

fn text<'a>(entry: &'a serde_json::Value, key: &str) -> &'a str {
    entry[key].as_str().expect("语料文本字段")
}

fn rows(entry: &serde_json::Value, key: &str) -> Vec<Vec<String>> {
    entry[key]
        .as_array()
        .expect("语料行矩阵")
        .iter()
        .map(|r| {
            r.as_array()
                .expect("行")
                .iter()
                .map(|c| c.as_str().expect("单元格").to_string())
                .collect()
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 1. 解析层：Python csv oracle 对拍
// ---------------------------------------------------------------------------

/// 全部语料的解析一致（逐单元格；header + 数据行整体矩阵）
#[test]
fn parse_matches_python_csv_oracle() {
    for entry in fixtures() {
        let name = entry["name"].as_str().unwrap();
        if entry["expectError"].as_bool().unwrap() {
            // 未闭合引号：ai-core 严格报错（Python 宽松读尾——见生成器说明）
            let e = parse_table(text(&entry, "left"), &TableParseOptions::default()).unwrap_err();
            assert!(format!("{e}").contains("限定符未闭合"), "{name}: {e}");
            continue;
        }
        let opts = TableParseOptions {
            delimiter: if name.contains("_tsv_") || name.contains("tsv") { '\t' } else { ',' },
            ..TableParseOptions::default()
        };
        let left = parse_table(text(&entry, "left"), &opts)
            .unwrap_or_else(|e| panic!("{name}: 解析失败 {e}"));
        let right = parse_table(text(&entry, "right"), &opts)
            .unwrap_or_else(|e| panic!("{name}: 解析失败 {e}"));

        // 全矩阵一致（含 header；TSV 语料 header 也参与矩阵）
        let expected_left = rows(&entry, "leftRows");
        let expected_right = rows(&entry, "rightRows");
        let actual_left: Vec<Vec<String>> = {
            let mut all = vec![left.header.clone()];
            all.extend(left.rows.clone());
            all
        };
        let actual_right: Vec<Vec<String>> = {
            let mut all = vec![right.header.clone()];
            all.extend(right.rows.clone());
            all
        };
        assert_eq!(actual_left, expected_left, "{name}: 左侧解析与 Python csv oracle 不一致");
        assert_eq!(actual_right, expected_right, "{name}: 右侧解析与 Python csv oracle 不一致");
    }
}

// ---------------------------------------------------------------------------
// 2. diff 语义 golden
// ---------------------------------------------------------------------------

fn kinds(model: &ai_core::table::TableDiffModel) -> Vec<&'static str> {
    model.rows.iter().map(|r| r.kind.as_str()).collect()
}

fn statuses(model: &ai_core::table::TableDiffModel, row: usize) -> Vec<&'static str> {
    model.rows[row].cells.iter().map(|c| c.status.as_str()).collect()
}

#[test]
fn golden_01_basic_change() {
    let e = &fixtures()[0];
    let m = diff_tables(text(e, "left"), text(e, "right"), &TableDiffOptions::default()).unwrap();
    assert_eq!(kinds(&m), vec!["same", "changed"]);
    assert_eq!(statuses(&m, 1), vec!["same", "same", "importantDiff"]);
    assert!(m.rows[1].important);
}

#[test]
fn golden_04_embedded_newline_rows() {
    let e = &fixtures()[3];
    let m = diff_tables(text(e, "left"), text(e, "right"), &TableDiffOptions::default()).unwrap();
    assert_eq!(kinds(&m), vec!["same", "added"], "内嵌换行单元格整体为单行");
}

#[test]
fn golden_05_bom_semantics() {
    let e = &fixtures()[4];
    let m = diff_tables(text(e, "left"), text(e, "right"), &TableDiffOptions::default()).unwrap();
    assert_eq!(kinds(&m), vec!["same"], "BOM 剥离后两侧全同");
    assert_eq!(m.columns[0].name, "id", "BOM 不污染表头列名");
}

#[test]
fn golden_07_ragged_and_empty() {
    let e = &fixtures()[6];
    let m = diff_tables(text(e, "left"), text(e, "right"), &TableDiffOptions::default()).unwrap();
    assert_eq!(kinds(&m), vec!["same", "same"], "松散行短列取空语义稳定");
}

#[test]
fn golden_08_unicode() {
    let e = &fixtures()[7];
    let m = diff_tables(text(e, "left"), text(e, "right"), &TableDiffOptions::default()).unwrap();
    assert_eq!(kinds(&m), vec!["same", "changed"]);
    assert_eq!(m.rows[1].cells[1].left.as_deref(), Some("京都"));
    assert_eq!(m.rows[1].cells[1].right.as_deref(), Some("大阪"));
}

#[test]
fn golden_09_duplicate_keys_pair_in_order() {
    let e = &fixtures()[8];
    let m = diff_tables(
        text(e, "left"),
        text(e, "right"),
        &TableDiffOptions { key_columns: vec![0], ..Default::default() },
    )
    .unwrap();
    assert_eq!(kinds(&m), vec!["changed", "changed", "removed"], "键重复按出现序配对");
}

#[test]
fn golden_10_column_reorder_by_name() {
    let e = &fixtures()[9];
    let m = diff_tables(
        text(e, "left"),
        text(e, "right"),
        &TableDiffOptions { column_align: ColumnAlign::ByLeftName, ..Default::default() },
    )
    .unwrap();
    // 右侧缺 id 列 -> 每行 id 单元格 leftOnly（重要差异）
    assert_eq!(kinds(&m), vec!["changed"]);
    assert_eq!(statuses(&m, 0), vec!["leftOnly", "same", "same"]);
    let names: Vec<&str> = m.columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, vec!["id", "qty", "name"]);
    assert_eq!(m.columns[2].left, Some(2));
    assert_eq!(m.columns[2].right, Some(0));
}

#[test]
fn golden_11_missing_column_is_left_only() {
    let e = &fixtures()[10];
    let m = diff_tables(
        text(e, "left"),
        text(e, "right"),
        &TableDiffOptions { column_align: ColumnAlign::ByLeftName, ..Default::default() },
    )
    .unwrap();
    assert_eq!(kinds(&m), vec!["changed"], "extra 列单侧 -> 每行均有 leftOnly 单元格");
    assert_eq!(statuses(&m, 0), vec!["same", "same", "leftOnly"]);
    assert!(m.rows[0].important, "单侧列算重要差异");
}

#[test]
fn golden_12_empty_cells_roundtrip() {
    let e = &fixtures()[11];
    let m = diff_tables(text(e, "left"), text(e, "right"), &TableDiffOptions::default()).unwrap();
    assert_eq!(kinds(&m), vec!["same", "same", "added"]);
}

#[test]
fn golden_13_numeric_importance_p10_option() {
    let e = &fixtures()[12];
    let strict = TableDiffOptions::default();
    let m = diff_tables(text(e, "left"), text(e, "right"), &strict).unwrap();
    assert_eq!(statuses(&m, 1)[1], "importantDiff", "默认无容差");
    // P10 预留的数值容差：0.2 内判不重要（蓝点）
    let tolerant = TableDiffOptions {
        importance: TableImportance { numeric_tolerance: Some(0.2), ..Default::default() },
        ..Default::default()
    };
    let m = diff_tables(text(e, "left"), text(e, "right"), &tolerant).unwrap();
    assert_eq!(statuses(&m, 1)[1], "unimportantDiff");
    assert!(!m.rows[1].important);
}

#[test]
fn golden_14_case_unimportant_blue_dot() {
    let e = &fixtures()[13];
    let strict = TableDiffOptions::default();
    let m = diff_tables(text(e, "left"), text(e, "right"), &strict).unwrap();
    assert!(m.rows[0].important, "默认大小写重要（红点）");
    let iu = TableDiffOptions {
        importance: TableImportance { case_unimportant: true, ..Default::default() },
        ..Default::default()
    };
    let m = diff_tables(text(e, "left"), text(e, "right"), &iu).unwrap();
    assert_eq!(kinds(&m), vec!["changed"]);
    assert!(!m.rows[0].important, "-iu 语义：仅大小写差异 -> 蓝点");
    assert_eq!(statuses(&m, 0)[1], "unimportantDiff");
}

#[test]
fn golden_15_whitespace_unimportant() {
    let e = &fixtures()[14];
    let iu = TableDiffOptions {
        importance: TableImportance { whitespace: WhitespaceMode::IgnoreAll, ..Default::default() },
        ..Default::default()
    };
    let m = diff_tables(text(e, "left"), text(e, "right"), &iu).unwrap();
    assert!(!m.rows[0].important, "尾随空白不重要 -> 蓝点");
}

#[test]
fn golden_17_tsv_diff() {
    let e = &fixtures()[16];
    let opts = TableParseOptions { delimiter: '\t', ..Default::default() };
    let l = parse_table(text(e, "left"), &opts).unwrap();
    let r = parse_table(text(e, "right"), &opts).unwrap();
    let m = diff_table_data(&l, &r, &TableDiffOptions::default());
    assert_eq!(kinds(&m), vec!["same", "changed"]);
    assert!(m.rows[1].important);
}

#[test]
fn golden_18_added_removed_shape() {
    let e = &fixtures()[17];
    let m = diff_tables(
        text(e, "left"),
        text(e, "right"),
        &TableDiffOptions { row_align: RowAlign::Standard, ..Default::default() },
    )
    .unwrap();
    assert_eq!(kinds(&m), vec!["same", "removed", "same", "added", "added"], "Standard 行对齐");
    assert_eq!(m.rows[1].left, Some(1));
    assert_eq!(m.rows[1].right, None);
    assert_eq!(m.rows[3].right, Some(2), "首个 added 行 = 右侧行 2");
}

#[test]
fn golden_unaligned_row_mode() {
    let e = &fixtures()[17];
    let m = diff_tables(
        text(e, "left"),
        text(e, "right"),
        &TableDiffOptions { row_align: RowAlign::Unaligned, ..Default::default() },
    )
    .unwrap();
    // 不对齐（FR-TBL-004）：按行号位置配对（0↔0, 1↔1, 2↔2）+ 右侧余量 added
    assert_eq!(kinds(&m), vec!["same", "changed", "changed", "added"]);
}

#[test]
fn golden_skew_fallback_bounded() {
    // 键交叉（monotone 阻塞）：skew=1 内容补配；skew=0 不补配
    let left = "id,v\n1,a\n2,b\n3,c\n4,d\n5,e\n";
    let right = "id,v\n2,b\n3,c\n4,d\n5,e\n1,a\n";
    let m0 = diff_tables(left, right, &TableDiffOptions { key_columns: vec![0], skew_tolerance: 0, ..Default::default() }).unwrap();
    assert_eq!(m0.summary.removed_rows, 4);
    assert_eq!(m0.summary.added_rows, 4);
    let m1 = diff_tables(left, right, &TableDiffOptions { key_columns: vec![0], skew_tolerance: 1, ..Default::default() }).unwrap();
    assert_eq!(kinds(&m1), vec!["same", "same", "same", "same", "same"], "偏斜容差 1 补配全部");
}

// ---------------------------------------------------------------------------
// 3. 单元格比较（类型推断 + 数值/日期列）
// ---------------------------------------------------------------------------

#[test]
fn cell_compare_numeric_and_date_columns() {
    assert_eq!(
        compare_cell("1.0", "1.000", ColumnType::Number, &TableImportance::default()),
        CellDiffStatus::ImportantDiff,
        "数值列按数值语义：字面不同但值相同 -> 键折叠后等价；此处直接比较仍是 Important（无容差）"
    );
    let imp = TableImportance { numeric_tolerance: Some(0.001), ..Default::default() };
    assert_eq!(compare_cell("1.0", "1.000", ColumnType::Number, &imp), CellDiffStatus::UnimportantDiff);
    assert_eq!(compare_cell("2024-01-01", "2024-01-01", ColumnType::Date, &imp), CellDiffStatus::Same);
    assert_eq!(compare_cell("x", "x", ColumnType::Text, &imp), CellDiffStatus::Same);
}

/// 类型推断覆盖：Number/Date/Text（宽松；13 语料的 score 列应为 Number）
#[test]
fn column_type_inference_across_corpora() {
    let e = &fixtures()[12];
    let l = parse_table(text(e, "left"), &TableParseOptions::default()).unwrap();
    let r = parse_table(text(e, "right"), &TableParseOptions::default()).unwrap();
    let m = diff_table_data(&l, &r, &TableDiffOptions::default());
    assert_eq!(m.columns[1].ty, ColumnType::Number, "score 列推断为数值");
}
