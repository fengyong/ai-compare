#!/usr/bin/env python3
"""生成算法对拍 golden 矩阵 fixtures（docs/04 P1 交付物 ⑥）。

对每个语料用例，用两套独立实现计算期望 hunks：
- Python difflib.SequenceMatcher（autojunk=False）的 opcodes；
- git diff --no-index -U0 的 hunk 头。

hunk 表示：[left_start, left_end, right_start, right_end]（0 起半开，
与 ai-core section 区间同约定）。

exact=true 的用例要求 ai-core 四种算法（Standard/Myers/Patience/Histogram）
的 hunks 与 difflib、git 完全一致（语料设计为对齐无歧义）；
exact=false 的用例（移动块/高重复）只做性质校验（重放还原 + 无假等值 +
无缝覆盖），因为不同算法允许产出不同（但同样正确）的对齐。

运行：python3 tests/algodiff_matrix/generate_fixtures.py
输出：tests/algodiff_matrix/fixtures.json
"""

import difflib
import json
import re
import subprocess
import tempfile
from pathlib import Path

HERE = Path(__file__).parent


def difflib_hunks(a_lines, b_lines):
    sm = difflib.SequenceMatcher(None, a_lines, b_lines, autojunk=False)
    out = []
    for tag, i1, i2, j1, j2 in sm.get_opcodes():
        if tag != "equal":
            out.append([i1, i2, j1, j2])
    return out


GIT_HUNK = re.compile(r"^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@")


def parse_range(start, count):
    """git -U0 头转 0 起半开区间；count 缺省为 1，count=0 表示插入点。"""
    start = int(start)
    count = 1 if count is None else int(count)
    if count == 0:
        return [start, start]
    return [start - 1, start - 1 + count]


def git_hunks(left_text, right_text):
    with tempfile.TemporaryDirectory() as td:
        lp = Path(td) / "left.txt"
        rp = Path(td) / "right.txt"
        lp.write_text(left_text, encoding="utf-8")
        rp.write_text(right_text, encoding="utf-8")
        proc = subprocess.run(
            ["git", "diff", "--no-index", "-U0", "--no-color", str(lp), str(rp)],
            capture_output=True,
            text=True,
        )
    out = []
    for line in proc.stdout.splitlines():
        m = GIT_HUNK.match(line)
        if m:
            ls, le = parse_range(m.group(1), m.group(2))
            rs, re_ = parse_range(m.group(3), m.group(4))
            out.append([ls, le, rs, re_])
    return out


def norm_lines(text):
    """与 ai-core 默认骨架键一致的行序列：剥离行尾**风格**（LF/CRLF/CR），
    但保留末行**有无**行尾符（git/diff3 的 "\\ No newline at end of file"
    是内容结构差异；早期版本用 splitlines() 把它抹掉，导致
    no_trailing_newline 用例的两套 oracle 都被喂了失真输入）。"""
    lines = text.splitlines()
    if lines and not text.endswith(("\n", "\r")):
        lines[-1] += "\x00none"
    return lines


def git_norm(text):
    """git 比较用文本：行尾统一为 \\n（剥离风格差异），末行缺行尾符则保留缺省。"""
    lines = text.splitlines()
    if not lines:
        return ""
    return "\n".join(lines) + ("\n" if text.endswith(("\n", "\r")) else "")


CASES = []


def case(name, left, right, exact, note=""):
    CASES.append(
        {
            "name": name,
            "left_raw": left,
            "right_raw": right,
            "exact": exact,
            "note": note,
            "difflib_hunks": difflib_hunks(norm_lines(left), norm_lines(right)),
            "git_hunks": git_hunks(git_norm(left), git_norm(right)),
        }
    )


# ---- 语料矩阵（docs/04：空、单行、全异、移动块、CRLF 混合、Unicode、超长行）----
case("empty_vs_empty", "", "", True)
case("empty_vs_lines", "", "a\nb\n", True)
case("lines_vs_empty", "a\nb\n", "", True)
case("single_line_equal", "hello\n", "hello\n", True)
case("single_line_changed", "hello\n", "hell\n", True)
case("all_different", "\n".join(f"left {i}" for i in range(10)) + "\n",
     "\n".join(f"right {i}" for i in range(10)) + "\n", True)
case("middle_insert", "one\ntwo\nfour\n", "one\ntwo\nthree\nfour\n", True)
case("middle_delete", "one\ntwo\nthree\nfour\n", "one\ntwo\nfour\n", True)
case("append", "a\nb\n", "a\nb\nc\nd\n", True)
case("prepend", "c\nd\n", "a\nb\nc\nd\n", True)
case("two_hunks", "a\nb\nc\nd\ne\nf\n", "a\nX\nc\nd\nY\nf\n", True)
case("insert_and_delete_apart", "keep1\ndel1\nkeep2\nkeep3\nkeep4\n",
     "keep1\nkeep2\nnew1\nkeep3\nkeep4\n", True)
case("replace_block", "h\np\nq\nr\ns\nt\n", "h\nP\nQ\nR\nS\nt\n", True)
case("moved_block", "1\n2\n3\n4\n5\n6\n7\n8\n", "1\n6\n7\n2\n3\n4\n5\n8\n",
     False, "移动块：各算法对齐合法但不唯一，只做性质校验")
case("crlf_mixed_no_diff", "a\r\nb\r\nc\r\n", "a\nb\r\nc\n", True,
     "默认忽略行尾风格 -> 无差异")
case("crlf_content_change", "a\r\nb\r\nc\r\n", "a\r\nB\r\nc\r\n", True)
case("unicode_change", "中文第一行\nemoji 🎉 line\nÜnïcödé\n",
     "中文第二行\nemoji 🎊 line\nÜnïcödé\n", True)
case("long_line_change", "x" * 5000 + "\n", "x" * 2500 + "y" + "x" * 2499 + "\n", True)
case("repeated_lines_shuffle", "x\ny\nx\ny\nx\n", "y\nx\ny\nx\ny\n",
     False, "高重复行：对齐不唯一，只做性质校验")
case("blank_line_orphan", "a\n\nb\n", "a\nb\n", True)
case("no_trailing_newline", "a\nb", "a\nb\n", True)

# 一致性自检：exact 用例的两套参考实现必须互相一致（否则语料有歧义，需调整）
for c in CASES:
    if c["exact"]:
        assert c["difflib_hunks"] == c["git_hunks"], (
            f"{c['name']}: difflib 与 git 对齐不一致，语料存在歧义\n"
            f"  difflib={c['difflib_hunks']}\n  git={c['git_hunks']}"
        )

out = {"cases": CASES}
(HERE / "fixtures.json").write_text(json.dumps(out, ensure_ascii=False, indent=1), encoding="utf-8")
print(f"生成 {len(CASES)} 个用例 -> {HERE / 'fixtures.json'}")
for c in CASES:
    print(f"  {c['name']:<28} exact={c['exact']!s:<5} hunks={len(c['difflib_hunks'])}")
