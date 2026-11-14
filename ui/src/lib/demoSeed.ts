import { joinPath } from './env';
import type { KvFileStore } from '../session/storageAdapter';

/**
 * 演示数据目录（真实引擎模式下的「填充示例路径」落盘清单）。
 *
 * 背景：内置演示引擎（mock）不触碰文件系统，任意路径都能渲染一棵合成目录树，
 * 旧实现因此用占位路径 `/demo/project-v1`；桌面版走真实引擎后 `dir/open` 会真的
 * 扫描文件系统，占位路径必然报「根路径不存在（left）」。故真实引擎模式下改为在
 * 应用数据目录落盘一对**真实**演示目录，内容覆盖四种状态：
 * 相同 / 不同 / 仅左 / 仅右。
 */

export interface DemoFileSpec {
  /** 相对演示根的 POSIX 路径 */
  rel: string;
  contents: string;
}

/** 左侧演示目录（project-v1）文件清单 */
export const DEMO_LEFT_FILES: DemoFileSpec[] = [
  { rel: 'README.md', contents: '# demo project\n\nai-compare 演示数据。\n' },
  { rel: 'src/main.rs', contents: 'fn main() {\n    println!("hello, v1");\n}\n' },
  {
    rel: 'src/util.rs',
    contents: 'pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n',
  },
  { rel: 'config.json', contents: '{\n  "version": 1,\n  "features": ["diff"]\n}\n' },
  { rel: 'docs/only-left.md', contents: '# 仅左侧存在的文档\n' },
  { rel: 'legacy/old.txt', contents: 'legacy\n' },
];

/** 右侧演示目录（project-v2）文件清单 */
export const DEMO_RIGHT_FILES: DemoFileSpec[] = [
  { rel: 'README.md', contents: '# demo project\n\nai-compare 演示数据。\n' },
  {
    rel: 'src/main.rs',
    contents: 'fn main() {\n    println!("hello, v2");\n    println!("extra line");\n}\n',
  },
  {
    rel: 'src/util.rs',
    contents: 'pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n',
  },
  { rel: 'src/new.rs', contents: 'pub fn new_helper() -> bool {\n    true\n}\n' },
  { rel: 'config.json', contents: '{\n  "version": 2,\n  "features": ["diff", "merge"]\n}\n' },
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

/** 落盘清单：两侧全部文件（绝对路径 + 内容） */
export function demoFilePlan(baseDir: string): { path: string; contents: string }[] {
  const { left, right } = demoRoots(baseDir);
  const out: { path: string; contents: string }[] = [];
  for (const f of DEMO_LEFT_FILES) {
    out.push({ path: joinPath(left, f.rel), contents: f.contents });
  }
  for (const f of DEMO_RIGHT_FILES) {
    out.push({ path: joinPath(right, f.rel), contents: f.contents });
  }
  return out;
}

/**
 * 在磁盘上落盘演示目录（真实引擎模式）。
 *
 * 写前先删同名文件与 `.bak`：Rust 壳的 `settings_write` 在目标已存在时会把旧文件
 * 备份为 `<name>.bak`，不预删的话重复生成会在演示目录里留下 `.bak`，污染目录 diff。
 */
export async function seedDemoDirs(
  baseDir: string,
  store: KvFileStore,
): Promise<{ left: string; right: string }> {
  for (const f of demoFilePlan(baseDir)) {
    await store.deleteFile(f.path);
    await store.writeFile(f.path, f.contents);
  }
  return demoRoots(baseDir);
}
