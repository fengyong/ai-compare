import { beforeEach, describe, expect, it } from 'vitest';
import { MemoryFileStore } from './storageAdapter';
import { SETTINGS_FILES } from './storage';
import { emptySessionsDoc, parseSessionsDoc, serializeDoc } from './model';
import { LockedSessionError, SessionService } from './service';

const S_PATH = SETTINGS_FILES.sessions;

async function makeService(files?: Map<string, string>): Promise<{ svc: SessionService; store: MemoryFileStore }> {
  const store = new MemoryFileStore();
  if (files) {
    for (const [k, v] of files) (store as unknown as { files: Map<string, string> }).files.set(k, v);
  }
  const svc = new SessionService(store);
  await svc.init();
  return { svc, store };
}

function folderCompareInput(name = '发布目录对比'): {
  kind: 'folder-compare';
  name: string;
  specs: { left: string; right: string };
} {
  return { kind: 'folder-compare', name, specs: { left: '/tmp/L', right: '/tmp/R' } };
}

describe('session/SessionService', () => {
  beforeEach(() => {
    // 每用例独立的 store/service，不共享单例状态
  });

  it('init：全新安装四文档以空文档起步', async () => {
    const { svc } = await makeService();
    expect(svc.report.recoveredFromBackup).toEqual([]);
    expect(svc.report.lost).toEqual([]);
    expect(svc.getTree().sessions.size).toBe(0);
    expect(svc.getGlobalSettings()).toEqual({});
  });

  it('saveSession 创建 → 持久化 → 重新 init 可读回', async () => {
    const { svc, store } = await makeService();
    const node = await svc.saveSession(folderCompareInput());
    expect(node.id).toBeTruthy();
    expect(store.peek(S_PATH)).toContain('发布目录对比');

    const store2 = new MemoryFileStore();
    (store2 as unknown as { files: Map<string, string> }).files.set(S_PATH, store.peek(S_PATH)!);
    const svc2 = new SessionService(store2);
    await svc2.init();
    expect(svc2.listSessions()).toHaveLength(1);
    expect(svc2.listSessions()[0].specs.left).toBe('/tmp/L');
  });

  it('saveSession 更新已有会话（updatedAt 前进）', async () => {
    const { svc } = await makeService();
    const node = await svc.saveSession(folderCompareInput());
    await new Promise((r) => setTimeout(r, 5));
    const updated = await svc.saveSession({ ...folderCompareInput('改名'), specs: { left: '/x', right: '/y' } }, node.id);
    expect(updated.name).toBe('改名');
    expect(updated.specs.left).toBe('/x');
    expect(updated.updatedAt > node.updatedAt).toBe(true);
    expect(svc.listSessions()).toHaveLength(1);
  });

  it('saveAsSession：复制为新会话，默认名加副本', async () => {
    const { svc } = await makeService();
    const a = await svc.saveSession(folderCompareInput());
    const b = await svc.saveAsSession(a.id);
    expect(b.id).not.toBe(a.id);
    expect(b.name).toBe('发布目录对比 副本');
    expect(b.specs).toEqual(a.specs);
    expect(svc.listSessions()).toHaveLength(2);
  });

  it('renameSession / deleteSession（含树 children 清理与 MRU 清理）', async () => {
    const { svc, store } = await makeService();
    const fid = await svc.createFolder('工作');
    const node = await svc.saveSession({ ...folderCompareInput(), folderId: fid });
    await svc.recordOpened(node.kind, node.specs, node.id);
    expect(svc.getMruSessions()).toEqual([node.id]);

    await svc.renameSession(node.id, '新名字');
    expect(svc.getSession(node.id)?.name).toBe('新名字');
    // 树 children 同步
    const tree = svc.getTree();
    expect(tree.folders.get(fid)?.children).toContain(node.id);

    await svc.deleteSession(node.id);
    expect(svc.getSession(node.id)).toBeUndefined();
    expect(svc.getTree().folders.get(fid)?.children).not.toContain(node.id);
    expect(svc.getMruSessions()).toEqual([]);
    expect(store.peek(S_PATH)).not.toContain('新名字');
  });

  it('锁定会话：改名/保存/状态写回被拒绝（SES-001）', async () => {
    const { svc } = await makeService();
    const node = await svc.saveSession(folderCompareInput());
    await svc.setLocked(node.id, true);
    expect(svc.getSession(node.id)?.locked).toBe(true);

    await expect(svc.renameSession(node.id, 'x')).rejects.toThrow(LockedSessionError);
    await expect(svc.saveSession(folderCompareInput('x'), node.id)).rejects.toThrow(LockedSessionError);
    await svc.saveSessionState(node.id, { displayFilter: ['diff'] });
    expect(svc.getSession(node.id)?.state.displayFilter).toBeUndefined(); // 状态跳过
    await expect(
      svc.writeOverride('session', node.kind, node.id, { compare: { mtimeToleranceSec: 1 } })
    ).rejects.toThrow(LockedSessionError);
    await svc.setLocked(node.id, false);
    await svc.saveSessionState(node.id, { displayFilter: ['diff'] });
    expect(svc.getSession(node.id)?.state.displayFilter).toEqual(['diff']);
  });

  it('MRU 路径：按类型分列、去重置前、上限截断', async () => {
    const { svc } = await makeService();
    for (let i = 0; i < 12; i++) {
      await svc.recordOpened('folder-compare', { left: `/L${i}`, right: `/R${i}` });
    }
    const mru = svc.getMruPaths('folder-compare');
    expect(mru.left[0]).toBe('/L11');
    expect(mru.left.length).toBeLessThanOrEqual(10);
    expect(svc.getMruPaths('text-compare').left).toEqual([]);
    await svc.recordOpened('folder-compare', { left: '/L5', right: '/R5' });
    expect(svc.getMruPaths('folder-compare').left[0]).toBe('/L5');
  });

  it('作用域写回：session / kind / global 三层各自落位并合并生效', async () => {
    const { svc } = await makeService();
    const node = await svc.saveSession(folderCompareInput());

    await svc.writeOverride('global', node.kind, null, { compare: { mtimeToleranceSec: 2, contentCompare: 'auto' } });
    await svc.writeOverride('kind', node.kind, null, { compare: { mtimeToleranceSec: 5 } });
    await svc.writeOverride('session', node.kind, node.id, { compare: { contentCompare: 'always' } });

    const eff = svc.resolveSessionEffective(node.kind, svc.getSession(node.id)?.overrides);
    expect(eff.values.compare).toEqual({ mtimeToleranceSec: 5, contentCompare: 'always' });
    expect(eff.origin.compare).toBe('session');
  });

  it('作用域写回 kind 层影响其它新会话的生效值（SES-003）', async () => {
    const { svc } = await makeService();
    const a = await svc.saveSession(folderCompareInput('A'));
    await svc.writeOverride('kind', a.kind, null, { display: { showOrphans: false } });
    const b = await svc.saveSession(folderCompareInput('B'));
    const eff = svc.resolveSessionEffective(b.kind, b.overrides);
    expect(eff.values.display).toEqual({ showOrphans: false });
    expect(eff.origin.display).toBe('kind');
  });

  it('parent-* 作用域在服务层拒绝（需父会话上下文）', async () => {
    const { svc } = await makeService();
    const node = await svc.saveSession(folderCompareInput());
    await expect(
      svc.writeOverride('parent-pair', node.kind, node.id, { compare: { mtimeToleranceSec: 1 } })
    ).rejects.toThrow(/父会话/);
    await expect(
      svc.writeOverride('parent-kind', node.kind, null, { compare: { mtimeToleranceSec: 1 } })
    ).rejects.toThrow(/父会话/);
  });

  it('损坏的 sessions.json → .bak 自动回滚 + 恢复报告 + 主文件回写修复', async () => {
    const good = serializeDoc(emptySessionsDoc());
    const files = new Map<string, string>([
      [S_PATH, '{"version":1,"sessions":[坏掉'],
      [`${S_PATH}.bak`, good],
    ]);
    const { svc, store } = await makeService(files);
    expect(svc.report.recoveredFromBackup).toEqual(['sessions.json']);
    expect(svc.getTree().sessions.size).toBe(0);
    // 回写修复：主文件与 .bak 一致（再次 init 直读主文件）
    expect(store.peek(S_PATH)).toBe(good);
    const store2 = new MemoryFileStore();
    (store2 as unknown as { files: Map<string, string> }).files.set(S_PATH, store.peek(S_PATH)!);
    const svc2 = new SessionService(store2);
    await svc2.init();
    expect(svc2.report.recoveredFromBackup).toEqual([]);
  });

  it('主/备均损坏 → lost 报告 + 空文档起步（不崩溃）', async () => {
    const files = new Map<string, string>([
      [S_PATH, 'broken'],
      [`${S_PATH}.bak`, 'also broken'],
    ]);
    const { svc } = await makeService(files);
    expect(svc.report.lost).toHaveLength(1);
    expect(svc.report.lost[0].file).toBe('sessions.json');
    expect(svc.listSessions()).toEqual([]);
    // 之后仍可正常保存
    const node = await svc.saveSession(folderCompareInput());
    expect(svc.getSession(node.id)?.name).toBe('发布目录对比');
  });

  it('createFolder 嵌套与根挂载；非空文件夹删除被拒绝', async () => {
    const { svc } = await makeService();
    const root = await svc.createFolder('工作');
    const child = await svc.createFolder('发布', root);
    const tree = svc.getTree();
    expect(tree.rootFolderIds).toContain(root);
    expect(tree.folders.get(root)?.children).toContain(child);
    expect(tree.rootFolderIds).not.toContain(child);

    // 非空拒绝
    await expect(svc.deleteFolder(root)).rejects.toThrow(/非空/);
    await expect(svc.deleteFolder(child)).resolves.toBeUndefined();
    await expect(svc.deleteFolder(root)).resolves.toBeUndefined();
    expect(svc.getTree().folders.size).toBe(0);
  });

  it('emit：变更通知订阅者', async () => {
    const { svc } = await makeService();
    let fired = 0;
    svc.subscribe(() => fired++);
    await svc.saveSession(folderCompareInput());
    expect(fired).toBeGreaterThan(0);
  });

  it('会话文档结构对齐 docs/03 §4.3 示例（folders/sessions/rootFolderIds）', async () => {
    const { svc } = await makeService();
    const fid = await svc.createFolder('工作');
    await svc.saveSession({ ...folderCompareInput(), folderId: fid });
    const raw = JSON.parse(serializeDoc(svc.debugSessionsDoc()));
    expect(Object.keys(raw)).toEqual(expect.arrayContaining(['version', 'folders', 'rootFolderIds', 'sessions']));
    expect(raw.version).toBe(1);
    expect(raw.sessions[0].overrides).toEqual({});
    expect(typeof raw.sessions[0].specs.left).toBe('string');
  });

  it('parseSessionsDoc 接受服务序列化输出（往返一致）', async () => {
    const { svc } = await makeService();
    await svc.saveSession(folderCompareInput());
    const text = serializeDoc(svc.debugSessionsDoc());
    const parsed = parseSessionsDoc(text);
    expect(parsed.sessions).toHaveLength(1);
  });
});
