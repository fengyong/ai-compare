import { describe, expect, it } from 'vitest';
import {
  DEMO_LEFT_FILES,
  DEMO_RIGHT_FILES,
  demoFilePlan,
  demoRoots,
  seedDemoDirs,
} from './demoSeed';
import { MemoryFileStore } from '../session/storageAdapter';

/**
 * 演示数据（真实引擎模式）：
 * 目录 diff 需覆盖四种状态 —— 相同 / 不同 / 仅左 / 仅右，
 * 否则用户点「填充示例路径」后看不到有意义的差异。
 */
describe('演示数据清单', () => {
  const byRel = (files: { rel: string; contents: string }[], rel: string) =>
    files.find((f) => f.rel === rel)?.contents;

  it('README 与 util.rs 两侧内容一致（diff 应显示为相同）', () => {
    expect(byRel(DEMO_LEFT_FILES, 'README.md')).toBe(byRel(DEMO_RIGHT_FILES, 'README.md'));
    expect(byRel(DEMO_LEFT_FILES, 'src/util.rs')).toBe(byRel(DEMO_RIGHT_FILES, 'src/util.rs'));
  });

  it('main.rs 与 config.json 两侧内容不同（diff 应显示为不同）', () => {
    expect(byRel(DEMO_LEFT_FILES, 'src/main.rs')).not.toBe(byRel(DEMO_RIGHT_FILES, 'src/main.rs'));
    expect(byRel(DEMO_LEFT_FILES, 'config.json')).not.toBe(byRel(DEMO_RIGHT_FILES, 'config.json'));
  });

  it('存在仅左 / 仅右条目（孤立与新增状态）', () => {
    expect(DEMO_LEFT_FILES.some((f) => f.rel === 'docs/only-left.md')).toBe(true);
    expect(DEMO_RIGHT_FILES.some((f) => f.rel === 'src/new.rs')).toBe(true);
    expect(DEMO_RIGHT_FILES.some((f) => f.rel === 'docs/only-left.md')).toBe(false);
  });
});

describe('演示目录路径', () => {
  it('两侧路径落在 <base>/demo 下且不产生重复斜杠', () => {
    const roots = demoRoots('/tmp/base');
    expect(roots.left).toBe('/tmp/base/demo/project-v1');
    expect(roots.right).toBe('/tmp/base/demo/project-v2');
    const roots2 = demoRoots('/tmp/base/');
    expect(roots2.left).toBe('/tmp/base/demo/project-v1');
  });

  it('落盘清单覆盖两侧全部文件', () => {
    const plan = demoFilePlan('/tmp/base');
    expect(plan.length).toBe(DEMO_LEFT_FILES.length + DEMO_RIGHT_FILES.length);
    expect(plan.every((f) => f.path.startsWith('/tmp/base/demo/'))).toBe(true);
  });
});

describe('seedDemoDirs', () => {
  it('写入两侧文件并返回演示目录路径', async () => {
    const store = new MemoryFileStore();
    const roots = await seedDemoDirs('/tmp/base', store);
    expect(roots.left).toBe('/tmp/base/demo/project-v1');
    const content = await store.readFile('/tmp/base/demo/project-v1/src/main.rs');
    expect(content).toContain('hello, v1');
  });

  it('重复生成不残留 .bak（写前先删同名文件与备份）', async () => {
    const store = new MemoryFileStore();
    await seedDemoDirs('/tmp/base', store);
    await seedDemoDirs('/tmp/base', store);
    expect(await store.readFile('/tmp/base/demo/project-v1/src/main.rs.bak')).toBeNull();
  });
});
