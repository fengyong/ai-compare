import { joinPath } from './env';
import type { BinaryFileStore, KvFileStore } from '../session/storageAdapter';
import { DEMO_ASSETS, type DemoAssetKey } from './demoAssets';

/**
 * 演示数据目录（真实引擎模式下的「填充示例路径」落盘清单）。
 *
 * 背景：内置演示引擎（mock）不触碰文件系统，任意路径都能渲染一棵合成目录树，
 * 旧实现因此用占位路径 `/demo/project-v1`；桌面版走真实引擎后 `dir/open` 会真的
 * 扫描文件系统，占位路径必然报「根路径不存在（left）」。故真实引擎模式下改为在
 * 应用数据目录落盘一对**真实**演示目录。
 *
 * 全格式覆盖（供体验全部 8 类视图）：
 * - 文本/代码：rs/py/ts/css/html/json/xml/yaml/toml/ini/sql/log/sh/Makefile/LICENSE；
 * - 表格：csv/tsv/tab（双击进表格视图）；
 * - 图片：png（内容差异 / 尺寸差异 / 相同 / 仅左；引擎 image crate 仅 png）；
 * - 压缩包：zip（zip:// 路由进归档比较）；
 * - 十六进制：dat/bin/o（双击进 hex 视图）；
 * - 编码：GB18030 / UTF-16LE+BOM（文本视图编码选择）；
 * - 行尾/BOM：CRLF vs LF、UTF-8 BOM vs 无 BOM；
 * - 结构：嵌套深层 / 空目录(.gitkeep) / 隐藏文件(.env/.gitignore/.hidden)；
 * - 四态：相同 / 不同 / 仅左 / 仅右；改名候选（old-name→new-name 同内容）；
 * - 三向合并：merge3/{base,left,right}.md（含冲突与可自动合并改动）；
 * - 大文件：large/big.txt（4000 行，右侧 3 处改动，测虚拟化与差异导航）。
 *
 * 二进制资产由 scripts/gen-demo-assets.mjs 生成（demoAssets.ts，base64），
 * 落盘时经 decodeAsset 还原为精确字节，走壳 settings_write_bytes（ confined 原子写）。
 */

export interface DemoFileSpec {
  /** 相对演示根的 POSIX 路径 */
  rel: string;
  /** 文本内容（与 asset 二选一） */
  contents?: string;
  /** 二进制资产键（与 contents 二选一） */
  asset?: DemoAssetKey;
}

/** base64 资产 → 精确字节（atob 在 Tauri WebView 与 Node 均可用） */
export function decodeAsset(key: DemoAssetKey): Uint8Array {
  const bin = atob(DEMO_ASSETS[key]);
  const bytes = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
  return bytes;
}

const isBinary = (f: DemoFileSpec): f is DemoFileSpec & { asset: DemoAssetKey } =>
  f.asset !== undefined;

// ------------------------------------------------------------------
// 文本内容（多格式）
// ------------------------------------------------------------------

const README = '# demo project\n\nai-compare 全格式演示数据。详见 DEMO-GUIDE.md。\n';

const GUIDE = `# 演示数据体验指南

本目录对（project-v1 ⇄ project-v2）覆盖全部 8 类视图与四种差异状态。
以下路径均相对各自演示根；「左」= project-v1，「右」= project-v2。

## 文件夹比较（默认）
- 四态：README.md / src/util.rs（相同）；src/main.rs / config.json（不同）；
  docs/only-left.md（仅左）；src/new.rs（仅右）。
- 改名候选：左 docs/old-name.md ⇄ 右 docs/new-name.md（内容相同）。
- 树形/平铺：工具栏切换；平铺下用「手动对齐」处理改名行。
- 隐藏文件：.env / .gitignore / .hidden/secret.txt（按隐藏文件开关显隐）。
- 嵌套与空目录：nested/deep/a/b/c.txt；nested/empty-left(.gitkeep 仅左)。
- 大文件：large/big.txt（4000 行，右侧 3 处改动；Ctrl+Down 跳差异）。

## 文本比较（双击文本文件）
- 代码：src/main.rs、src/app.py、src/index.ts。
- 行尾：text/crlf.txt（左 CRLF / 右 LF，逻辑内容相同）。
- BOM：text/bom.txt（左 UTF-8 BOM / 右无 BOM）。
- Unicode：text/unicode.txt（中文 + emoji，两侧相同）。
- 编码：encoding/gb18030.txt（GB18030）、encoding/utf16.txt（UTF-16LE+BOM）；
  在文本视图编码下拉手动选择对应编码查看正确中文。

## 表格比较（双击 csv/tsv/tab）
- data/users.csv、data/users.tsv（不同）；data/products.tab（相同）。

## 图片比较（双击 png）
- images/logo.png（内容差异）；images/banner.png（尺寸差异 120x40 vs 160x40）；
  images/icon.png（相同）；images/only-left.png（仅左）。

## 十六进制比较（双击 dat/bin/o）
- binary/blob.dat（不同）；binary/firmware.bin（仅左）；binary/lib.o（仅右）。

## 压缩包比较（zip:// 路由）
- 主页左右路径填：
  zip://<左绝对路径>/archives/data.zip!readme.txt
  zip://<右绝对路径>/archives/data.zip!readme.txt
  归档内含改名（src/b.txt→src/c.txt）与内容差异。

## 三向文本合并（主页「文本合并」）
- 基准/左/右分别填（均在左根 merge3/ 下）：
  <左绝对路径>/merge3/base.md
  <左绝对路径>/merge3/left.md
  <左绝对路径>/merge3/right.md
  name 两侧都改（冲突）；mode 仅右改、retry 仅左改（可自动合并）。

## 文件夹同步 / 文件夹合并
- 复用左右两根；文件夹合并的基准可留空（以左兼作）或填第三方目录。
`;

const MAIN_RS_L = 'fn main() {\n    println!("hello, v1");\n}\n';
const MAIN_RS_R = 'fn main() {\n    println!("hello, v2");\n    println!("extra line");\n}\n';
const UTIL_RS = 'pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n';
const NEW_RS = 'pub fn new_helper() -> bool {\n    true\n}\n';
const LEGACY_RS = 'pub fn deprecated() -> i32 {\n    0\n}\n';

const APP_PY_L = 'def greet(name):\n    return f"hello, {name}"\n';
const APP_PY_R = 'def greet(name: str) -> str:\n    return f"hi, {name}"\n';

const INDEX_TS_L = 'export const version = 1;\nexport function id<T>(x: T): T {\n  return x;\n}\n';
const INDEX_TS_R =
  'export const version = 2;\nexport function id<T>(x: T): T {\n  return x;\n}\nexport const extra = true;\n';

const STYLES_CSS = '.root {\n  color: #333;\n  font-size: 14px;\n}\n';
const PAGE_HTML_L = '<!doctype html>\n<html>\n  <body>\n    <p>v1</p>\n  </body>\n</html>\n';
const PAGE_HTML_R =
  '<!doctype html>\n<html>\n  <body>\n    <p>v2</p>\n    <hr />\n  </body>\n</html>\n';

const CONFIG_JSON_L = '{\n  "version": 1,\n  "features": ["diff"]\n}\n';
const CONFIG_JSON_R = '{\n  "version": 2,\n  "features": ["diff", "merge"]\n}\n';
const CONFIG_XML_L = '<config>\n  <version>1</version>\n</config>\n';
const CONFIG_XML_R = '<config>\n  <version>2</version>\n  <merge>true</merge>\n</config>\n';
const CONFIG_YAML_L = 'version: 1\nfeatures:\n  - diff\n';
const CONFIG_YAML_R = 'version: 2\nfeatures:\n  - diff\n  - merge\n';
const SETTINGS_TOML = '[server]\nport = 8080\nhost = "localhost"\n';
const APP_INI_L = '[app]\nname = demo\nmode = fast\n';
const APP_INI_R = '[app]\nname = demo\nmode = safe\n';

const USERS_CSV_L = 'id,name,role\n1,alice,admin\n2,bob,user\n';
const USERS_CSV_R = 'id,name,role\n1,alice,admin\n2,bob,editor\n3,carol,user\n';
const USERS_TSV_L = 'id\tname\trole\n1\talice\tadmin\n2\tbob\tuser\n';
const USERS_TSV_R = 'id\tname\trole\n1\talice\tadmin\n2\tbob\teditor\n';
const PRODUCTS_TAB = 'sku\tprice\tstock\nA1\t9.9\t100\nB2\t19.9\t50\n';

const SCHEMA_SQL_L = 'CREATE TABLE users (\n  id INTEGER PRIMARY KEY,\n  name TEXT\n);\n';
const SCHEMA_SQL_R =
  'CREATE TABLE users (\n  id INTEGER PRIMARY KEY,\n  name TEXT,\n  role TEXT\n);\n';
const SEED_SQL = "INSERT INTO users (id, name) VALUES (1, 'alice');\n";

const ONLY_LEFT_MD = '# 仅左侧存在的文档\n';
const ADDED_RIGHT_MD = '# 右侧新增的文档\n';
const DOCS_GUIDE = '# 文档\n\n两侧一致的说明文档。\n';
const RENAMED_MD = '# 改名候选\n\n本文件两侧内容相同，仅文件名不同（old-name → new-name）。\n';

const APP_LOG_L = '2026-01-01 INFO start\n2026-01-01 INFO done\n';
const APP_LOG_R = '2026-01-01 INFO start\n2026-01-01 WARN retry\n2026-01-01 INFO done\n';
const BUILD_SH = '#!/bin/sh\nset -e\necho build\n';
const DEPLOY_PS1 = 'Write-Host "deploy"\n';
const MAKEFILE = 'all:\n\techo build\n';
const LICENSE = 'MIT License\n\nCopyright (c) 2026 ai-compare\n';

const ENV_FILE = 'SECRET=demo\nDEBUG=1\n';
const GITIGNORE = 'target/\nnode_modules/\n*.bak\n';
const HIDDEN_SECRET = 'hidden content\n';

const CRLF_L = 'alpha\r\nbeta\r\ngamma\r\n';
const CRLF_R = 'alpha\nbeta\ngamma\n';
const BOM_L = '﻿bom line one\nbom line two\n';
const BOM_R = 'bom line one\nbom line two\n';
const UNICODE = '中文演示 🎉 café naïve\n第二行 ✅\n';

const NESTED_L = 'deep nested v1\n';
const NESTED_R = 'deep nested v2\n';
const GITKEEP = '';

const MERGE3_BASE = '# 配置\n\nname = demo\nmode = fast\nretry = 3\n\n# 结尾\n';
const MERGE3_LEFT = '# 配置\n\nname = demo-left\nmode = fast\nretry = 5\n\n# 结尾\n';
const MERGE3_RIGHT = '# 配置\n\nname = demo-right\nmode = safe\nretry = 3\n\n# 结尾\n';

/** 大文件（4000 行）：右侧 3 处改动，测虚拟化渲染与差异导航 */
function largeText(variant: 'left' | 'right'): string {
  const lines: string[] = [];
  for (let i = 1; i <= 4000; i++) {
    let line = `line ${String(i).padStart(5, '0')}: the quick brown fox jumps over the lazy dog`;
    if (variant === 'right' && (i === 100 || i === 2000 || i === 3999)) {
      line += '  [CHANGED]';
    }
    lines.push(line);
  }
  return lines.join('\n') + '\n';
}

// ------------------------------------------------------------------
// 文件清单
// ------------------------------------------------------------------

/** 左侧演示目录（project-v1）文件清单 */
export const DEMO_LEFT_FILES: DemoFileSpec[] = [
  { rel: 'README.md', contents: README },
  { rel: 'DEMO-GUIDE.md', contents: GUIDE },
  // 代码
  { rel: 'src/main.rs', contents: MAIN_RS_L },
  { rel: 'src/util.rs', contents: UTIL_RS },
  { rel: 'src/legacy.rs', contents: LEGACY_RS },
  { rel: 'src/app.py', contents: APP_PY_L },
  { rel: 'src/index.ts', contents: INDEX_TS_L },
  { rel: 'src/styles.css', contents: STYLES_CSS },
  { rel: 'src/page.html', contents: PAGE_HTML_L },
  // 配置
  { rel: 'config.json', contents: CONFIG_JSON_L },
  { rel: 'config.xml', contents: CONFIG_XML_L },
  { rel: 'config.yaml', contents: CONFIG_YAML_L },
  { rel: 'settings.toml', contents: SETTINGS_TOML },
  { rel: 'app.ini', contents: APP_INI_L },
  // 表格
  { rel: 'data/users.csv', contents: USERS_CSV_L },
  { rel: 'data/users.tsv', contents: USERS_TSV_L },
  { rel: 'data/products.tab', contents: PRODUCTS_TAB },
  // SQL
  { rel: 'db/schema.sql', contents: SCHEMA_SQL_L },
  { rel: 'db/seed.sql', contents: SEED_SQL },
  // 文档（四态 + 改名）
  { rel: 'docs/only-left.md', contents: ONLY_LEFT_MD },
  { rel: 'docs/guide.md', contents: DOCS_GUIDE },
  { rel: 'docs/old-name.md', contents: RENAMED_MD },
  // 日志 / 脚本 / 无扩展名
  { rel: 'logs/app.log', contents: APP_LOG_L },
  { rel: 'scripts/build.sh', contents: BUILD_SH },
  { rel: 'Makefile', contents: MAKEFILE },
  { rel: 'LICENSE', contents: LICENSE },
  // 隐藏文件 / 隐藏目录
  { rel: '.env', contents: ENV_FILE },
  { rel: '.gitignore', contents: GITIGNORE },
  { rel: '.hidden/secret.txt', contents: HIDDEN_SECRET },
  // 行尾 / BOM / Unicode
  { rel: 'text/crlf.txt', contents: CRLF_L },
  { rel: 'text/bom.txt', contents: BOM_L },
  { rel: 'text/unicode.txt', contents: UNICODE },
  // 编码（二进制资产）
  { rel: 'encoding/gb18030.txt', asset: 'gbL' },
  { rel: 'encoding/utf16.txt', asset: 'u16L' },
  // 图片（PNG）
  { rel: 'images/logo.png', asset: 'pngLogoL' },
  { rel: 'images/banner.png', asset: 'pngBannerL' },
  { rel: 'images/icon.png', asset: 'pngIcon' },
  { rel: 'images/only-left.png', asset: 'pngOnlyLeft' },
  // 压缩包
  { rel: 'archives/data.zip', asset: 'zipDataL' },
  // 十六进制 / 二进制
  { rel: 'binary/blob.dat', asset: 'blobL' },
  { rel: 'binary/firmware.bin', asset: 'blobL' },
  // 嵌套 / 空目录
  { rel: 'nested/deep/a/b/c.txt', contents: NESTED_L },
  { rel: 'nested/empty-left/.gitkeep', contents: GITKEEP },
  // 三向合并
  { rel: 'merge3/base.md', contents: MERGE3_BASE },
  { rel: 'merge3/left.md', contents: MERGE3_LEFT },
  { rel: 'merge3/right.md', contents: MERGE3_RIGHT },
  // 大文件
  { rel: 'large/big.txt', contents: largeText('left') },
];

/** 右侧演示目录（project-v2）文件清单 */
export const DEMO_RIGHT_FILES: DemoFileSpec[] = [
  { rel: 'README.md', contents: README },
  { rel: 'DEMO-GUIDE.md', contents: GUIDE },
  // 代码
  { rel: 'src/main.rs', contents: MAIN_RS_R },
  { rel: 'src/util.rs', contents: UTIL_RS },
  { rel: 'src/new.rs', contents: NEW_RS },
  { rel: 'src/app.py', contents: APP_PY_R },
  { rel: 'src/index.ts', contents: INDEX_TS_R },
  { rel: 'src/styles.css', contents: STYLES_CSS },
  { rel: 'src/page.html', contents: PAGE_HTML_R },
  // 配置
  { rel: 'config.json', contents: CONFIG_JSON_R },
  { rel: 'config.xml', contents: CONFIG_XML_R },
  { rel: 'config.yaml', contents: CONFIG_YAML_R },
  { rel: 'settings.toml', contents: SETTINGS_TOML },
  { rel: 'app.ini', contents: APP_INI_R },
  // 表格
  { rel: 'data/users.csv', contents: USERS_CSV_R },
  { rel: 'data/users.tsv', contents: USERS_TSV_R },
  { rel: 'data/products.tab', contents: PRODUCTS_TAB },
  // SQL
  { rel: 'db/schema.sql', contents: SCHEMA_SQL_R },
  { rel: 'db/seed.sql', contents: SEED_SQL },
  // 文档（四态 + 改名）
  { rel: 'docs/added-right.md', contents: ADDED_RIGHT_MD },
  { rel: 'docs/guide.md', contents: DOCS_GUIDE },
  { rel: 'docs/new-name.md', contents: RENAMED_MD },
  // 日志 / 脚本 / 无扩展名
  { rel: 'logs/app.log', contents: APP_LOG_R },
  { rel: 'scripts/build.sh', contents: BUILD_SH },
  { rel: 'scripts/deploy.ps1', contents: DEPLOY_PS1 },
  { rel: 'Makefile', contents: MAKEFILE },
  { rel: 'LICENSE', contents: LICENSE },
  // 隐藏文件 / 隐藏目录
  { rel: '.gitignore', contents: GITIGNORE },
  { rel: '.hidden/secret.txt', contents: HIDDEN_SECRET },
  // 行尾 / BOM / Unicode
  { rel: 'text/crlf.txt', contents: CRLF_R },
  { rel: 'text/bom.txt', contents: BOM_R },
  { rel: 'text/unicode.txt', contents: UNICODE },
  // 编码（二进制资产）
  { rel: 'encoding/gb18030.txt', asset: 'gbR' },
  { rel: 'encoding/utf16.txt', asset: 'u16R' },
  // 图片（PNG）
  { rel: 'images/logo.png', asset: 'pngLogoR' },
  { rel: 'images/banner.png', asset: 'pngBannerR' },
  { rel: 'images/icon.png', asset: 'pngIcon' },
  // 压缩包
  { rel: 'archives/data.zip', asset: 'zipDataR' },
  // 十六进制 / 二进制
  { rel: 'binary/blob.dat', asset: 'blobR' },
  { rel: 'binary/lib.o', asset: 'blobR' },
  // 嵌套 / 空目录
  { rel: 'nested/deep/a/b/c.txt', contents: NESTED_R },
  { rel: 'nested/empty-right/.gitkeep', contents: GITKEEP },
  // 三向合并
  { rel: 'merge3/base.md', contents: MERGE3_BASE },
  { rel: 'merge3/left.md', contents: MERGE3_LEFT },
  { rel: 'merge3/right.md', contents: MERGE3_RIGHT },
  // 大文件
  { rel: 'large/big.txt', contents: largeText('right') },
];

export const DEMO_LEFT_NAME = 'project-v1';
export const DEMO_RIGHT_NAME = 'project-v2';

/** 演示根目录（<应用数据目录>/demo） */
export function demoBase(baseDir: string): string {
  return joinPath(baseDir, 'demo');
}

/** 演示目录对的绝对路径 */
export function demoRoots(baseDir: string): { left: string; right: string } {
  const base = demoBase(baseDir);
  return { left: joinPath(base, DEMO_LEFT_NAME), right: joinPath(base, DEMO_RIGHT_NAME) };
}

/** 三向合并演示文件的绝对路径（均在左根 merge3/ 下） */
export function demoMerge3Paths(baseDir: string): { base: string; left: string; right: string } {
  const { left } = demoRoots(baseDir);
  return {
    base: joinPath(left, 'merge3/base.md'),
    left: joinPath(left, 'merge3/left.md'),
    right: joinPath(left, 'merge3/right.md'),
  };
}

export interface DemoPlanEntry {
  path: string;
  contents?: string;
  bytes?: Uint8Array;
}

/** 落盘清单：两侧全部文件（绝对路径 + 文本内容或二进制字节） */
export function demoFilePlan(baseDir: string): DemoPlanEntry[] {
  const { left, right } = demoRoots(baseDir);
  const out: DemoPlanEntry[] = [];
  const push = (root: string, f: DemoFileSpec) => {
    const path = joinPath(root, f.rel);
    if (isBinary(f)) out.push({ path, bytes: decodeAsset(f.asset) });
    else out.push({ path, contents: f.contents ?? '' });
  };
  for (const f of DEMO_LEFT_FILES) push(left, f);
  for (const f of DEMO_RIGHT_FILES) push(right, f);
  return out;
}

/**
 * 在磁盘上落盘演示目录（真实引擎模式）。
 *
 * 写前先删同名文件与 `.bak`：Rust 壳的 `settings_write`/`settings_write_bytes` 在目标
 * 已存在时会把旧文件备份为 `<name>.bak`，不预删的话重复生成会在演示目录里留下
 * `.bak`，污染目录 diff。文本走 writeFile，二进制走 writeBinaryFile。
 */
export async function seedDemoDirs(
  baseDir: string,
  store: KvFileStore & BinaryFileStore,
): Promise<{ left: string; right: string }> {
  for (const f of demoFilePlan(baseDir)) {
    await store.deleteFile(f.path);
    if (f.bytes !== undefined) await store.writeBinaryFile(f.path, f.bytes);
    else await store.writeFile(f.path, f.contents ?? '');
  }
  return demoRoots(baseDir);
}
