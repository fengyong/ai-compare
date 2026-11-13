#!/usr/bin/env python3
"""表格对拍 fixtures 生成器（P9 / docs/04 §P9 验收要点：CSV 边角语料）。

以 Python csv 模块（RFC4180 参考实现）为解析 oracle：每条语料给出
left/right 原文与 Python 解析结果（行矩阵）。Rust 侧 ai-core parse_table
的输出必须与 Python 一致（约定差异在 Python 侧归一）：

- 空白行：Python 产出 `[]` 空行，ai-core 直接跳过 —— 归一为两侧都跳过；
- UTF-8 BOM：ai-core 剥离首单元格 BOM —— 归一为 Python 侧也剥离；
- 未闭合引号：Python 宽松读到文件尾，ai-core 报错 —— 该语料不计入
  解析对拍（Rust 侧 golden 断言错误），corpus 带 `expect_error` 标记。

重新生成：`python3 tests/table_matrix/generate_fixtures.py`（写入本目录
fixtures.json）。diff 语义 golden（行类别/单元格状态）在 table_test.rs 内手写。
"""

import csv
import io
import json
import os

# 18 条语料（name, left, right, expect_error）
CORPORA = [
    ("01_basic_change",
     "id,name,qty\n1,Alpha,10\n2,Beta,20\n",
     "id,name,qty\n1,Alpha,10\n2,Beta,21\n",
     False),
    ("02_quoted_comma",
     'name,note\n"Alpha, Sr.",ok\n',
     'name,note\n"Alpha, Sr.",ok\n',
     False),
    ("03_quoted_quotes_nested",
     'name,nick\n"Bob ""The Builder""",B\n',
     'name,nick\n"Bob ""The Builder""",B\n',
     False),
    ("04_embedded_newline",
     'id,note\n1,"line1\nline2"\n',
     'id,note\n1,"line1\nline2"\n2,plain\n',
     False),
    ("05_bom_utf8",
     '﻿id,name\n1,Alpha\n',
     '﻿id,name\n1,Alpha\n',
     False),
    ("06_crlf_rows",
     "id,name\r\n1,Alpha\r\n2,Beta\r\n",
     "id,name\r\n1,Alpha\r\n2,Beta\r\n3,Gamma\r\n",
     False),
    ("07_empty_and_ragged",
     "a,b,c\n1\n1,2,3\n",
     "a,b,c\n1\n1,2,3\n",
     False),
    ("08_unicode_cells",
     "名前,都市\n太郎,東京\n花子,京都\n",
     "名前,都市\n太郎,東京\n花子,大阪\n",
     False),
    ("09_duplicate_keys",
     "id,v\n1,a\n1,b\n1,c\n",
     "id,v\n1,A\n1,B\n",
     False),
    ("10_column_reorder",
     "id,qty,name\n1,10,Alpha\n",
     "name,qty\nAlpha,10\n",
     False),
    ("11_missing_column",
     "id,name,extra\n1,Alpha,X\n",
     "id,name\n1,Alpha\n",
     False),
    ("12_empty_cells",
     "a,b,c\n1,,3\n,2,\n",
     "a,b,c\n1,,3\n,2,\n4,,\n",
     False),
    ("13_numeric_columns",
     "id,score\n1,3.14\n2,2.71\n",
     "id,score\n1,3.14\n2,2.72\n",
     False),
    ("14_case_only_diff",
     "id,name\n1,Alpha\n",
     "id,name\n1,ALPHA\n",
     False),
    ("15_whitespace_only_diff",
     "id,name\n1,Alpha  \n",
     "id,name\n1,Alpha\n",
     False),
    ("16_unterminated_quote",
     "a,b\n\"open\n",
     "a,b\n\"open\n",
     True),
    ("17_tsv_delimiter",
     "id\tname\n1\tAlpha\n2\tBeta\n",
     "id\tname\n1\tAlpha\n2\tBETA\n",
     False),
    ("18_added_removed_rows",
     "id,v\n1,keep\n2,drop\n3,move\n",
     "id,v\n1,keep\n3,move\n4,new\n5,new2\n",
     False),
]


def normalize(rows):
    """Python csv 输出 -> ai-core 语义（跳过空白行；BOM 剥离）。"""
    out = []
    for i, row in enumerate(rows):
        if row == []:
            continue  # ai-core 语义：真空白行跳过
        if i == 0 and row and row[0].startswith("﻿"):
            row = [row[0][1:]] + row[1:]  # ai-core 语义：BOM 剥离
        out.append(row)
    return out


def main():
    fixtures = []
    for name, left, right, expect_error in CORPORA:
        entry = {"name": name, "left": left, "right": right, "expectError": expect_error}
        if not expect_error:
            delim = "\t" if "tsv" in name else ","
            entry["leftRows"] = normalize(
                list(csv.reader(io.StringIO(left), delimiter=delim))
            )
            entry["rightRows"] = normalize(
                list(csv.reader(io.StringIO(right), delimiter=delim))
            )
        fixtures.append(entry)

    out_path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "fixtures.json")
    with open(out_path, "w", encoding="utf-8") as f:
        json.dump(fixtures, f, ensure_ascii=False, indent=1)
    print(f"wrote {out_path}: {len(fixtures)} corpora")


if __name__ == "__main__":
    main()
