import { describe, expect, it } from 'vitest';
import { BrowserFileStore, MemoryFileStore } from './storageAdapter';
import { recoverWithBackup } from './storage';

/**
 * R3-13：删除设置文件必须连 .bak 一起删。
 *
 * 否则删除后走双源恢复协议（主缺失 + .bak 存在 → 从 .bak 恢复）会把已删除的
 * 旧文档复活。Tauri 后端的同口径行为由 src-tauri 的 settings_delete 测试守。
 */

/** 最小 Storage 替身（浏览器后端在 node 环境无 localStorage） */
function fakeStorage(): Storage {
  const map = new Map<string, string>();
  return {
    get length() {
      return map.size;
    },
    clear: () => map.clear(),
    getItem: (k: string) => map.get(k) ?? null,
    key: (i: number) => [...map.keys()][i] ?? null,
    removeItem: (k: string) => void map.delete(k),
    setItem: (k: string, v: string) => void map.set(k, String(v)),
  } as Storage;
}

const parse = (text: string) => JSON.parse(text) as { v: number };

describe('MemoryFileStore：删除连 .bak 一起清', () => {
  it('两次写入产生 .bak；deleteFile 后主键与备份都不在', async () => {
    const store = new MemoryFileStore();
    await store.writeFile('/s.json', '{"v":1}');
    await store.writeFile('/s.json', '{"v":2}');
    expect(store.peek('/s.json')).toBe('{"v":2}');
    expect(store.peek('/s.json.bak')).toBe('{"v":1}');

    await store.deleteFile('/s.json');
    expect(store.peek('/s.json')).toBeNull();
    expect(store.peek('/s.json.bak')).toBeNull();
  });

  it('删除后恢复协议视为全新安装，而不是从 .bak 复活旧文档', async () => {
    const store = new MemoryFileStore();
    await store.writeFile('/s.json', '{"v":1}');
    await store.writeFile('/s.json', '{"v":2}');
    await store.deleteFile('/s.json');
    const r = recoverWithBackup(
      await store.readFile('/s.json'),
      await store.readFile('/s.json.bak'),
      parse,
    );
    expect(r.source).toBe('primary');
    expect(r.doc).toBeNull();
  });

  it('删除不存在的文件是幂等 no-op', async () => {
    const store = new MemoryFileStore();
    await store.deleteFile('/never.json');
    expect(store.peek('/never.json')).toBeNull();
  });
});

describe('BrowserFileStore：删除连 .bak 一起清', () => {
  it('两次写入产生 .bak 键；deleteFile 后两键都被移除', async () => {
    const storage = fakeStorage();
    const store = new BrowserFileStore(storage);
    await store.writeFile('s.json', '{"v":1}');
    await store.writeFile('s.json', '{"v":2}');
    expect(storage.getItem('aic:s.json.bak')).toBe('{"v":1}');

    await store.deleteFile('s.json');
    expect(storage.getItem('aic:s.json')).toBeNull();
    expect(storage.getItem('aic:s.json.bak')).toBeNull();
    expect(storage.length).toBe(0);
  });

  it('删除后恢复协议视为全新安装（浏览器口径与桌面一致）', async () => {
    const store = new BrowserFileStore(fakeStorage());
    await store.writeFile('s.json', '{"v":1}');
    await store.writeFile('s.json', '{"v":2}');
    await store.deleteFile('s.json');
    const r = recoverWithBackup(
      await store.readFile('s.json'),
      await store.readFile('s.json.bak'),
      parse,
    );
    expect(r.source).toBe('primary');
    expect(r.doc).toBeNull();
  });

  it('storage 为 null（无 localStorage）时删除不抛错', async () => {
    const store = new BrowserFileStore(null);
    await expect(store.deleteFile('s.json')).resolves.toBeUndefined();
  });
});
