import { beforeEach, describe, expect, it } from 'vitest';
import { resetShellForTest, useShell } from './store';

/**
 * 通用确认框与合并输出目标（R3-20，FR-TMRG-006）。
 *
 * 覆盖警示必须是"先问后写"的同步闸门：askConfirm 的 Promise 只在用户回答后
 * 结清，取消/重复弹出都不得让调用方永久挂起（否则保存流程静默卡死）。
 */

describe('askConfirm / resolveConfirm', () => {
  beforeEach(() => {
    resetShellForTest();
  });

  it('弹出后挂起，确认 → true 且对话框清空', async () => {
    const st = useShell.getState();
    const pending = st.askConfirm({ title: '覆盖目标文件？', message: '/tmp/out.txt' });
    expect(useShell.getState().confirm).toMatchObject({
      title: '覆盖目标文件？',
      message: '/tmp/out.txt',
    });
    useShell.getState().resolveConfirm(true);
    await expect(pending).resolves.toBe(true);
    expect(useShell.getState().confirm).toBeNull();
  });

  it('取消 → false（调用方据此不得写入）', async () => {
    const pending = useShell.getState().askConfirm({ title: 't', message: 'm' });
    useShell.getState().resolveConfirm(false);
    await expect(pending).resolves.toBe(false);
    expect(useShell.getState().confirm).toBeNull();
  });

  it('未决时再次弹出：前一个按取消结清，不永久挂起', async () => {
    const st = useShell.getState();
    const first = st.askConfirm({ title: 'first', message: 'm' });
    const second = st.askConfirm({ title: 'second', message: 'm' });
    await expect(first).resolves.toBe(false);
    expect(useShell.getState().confirm).toMatchObject({ title: 'second' });
    useShell.getState().resolveConfirm(true);
    await expect(second).resolves.toBe(true);
  });

  it('无对话框时 resolveConfirm 是 noop', () => {
    expect(() => useShell.getState().resolveConfirm(true)).not.toThrow();
    expect(useShell.getState().confirm).toBeNull();
  });
});

describe('setMerge3OutputPath', () => {
  beforeEach(() => {
    resetShellForTest();
  });

  it('保存后记录"合并到"目标，下次保存预填该路径', () => {
    const st = useShell.getState();
    st.openMerge3Tab('/b.txt', '/l.txt', '/r.txt');
    const tab = useShell.getState().windows[0].tabs.find((t) => t.kind === 'merge3');
    expect(tab?.merge3?.outputPath).toBeUndefined();
    st.setMerge3OutputPath(tab!.id, '/tmp/merged.txt');
    const after = useShell.getState().findTab(tab!.id)?.tab;
    expect(after?.merge3?.outputPath).toBe('/tmp/merged.txt');
    // 三路源文件与会话句柄不得被改写
    expect(after?.merge3).toMatchObject({ base: '/b.txt', left: '/l.txt', right: '/r.txt' });
    expect(after?.merge3?.sessionId).toBe(tab?.merge3?.sessionId);
  });

  it('只改指定标签，其他标签与 home 不受影响', () => {
    const st = useShell.getState();
    st.openMerge3Tab('/b1.txt', '/l1.txt', '/r1.txt');
    st.openMerge3Tab('/b2.txt', '/l2.txt', '/r2.txt');
    const tabs = useShell.getState().windows[0].tabs.filter((t) => t.kind === 'merge3');
    st.setMerge3OutputPath(tabs[0].id, '/tmp/first.txt');
    const now = useShell.getState().windows[0].tabs.filter((t) => t.kind === 'merge3');
    expect(now[0].merge3?.outputPath).toBe('/tmp/first.txt');
    expect(now[1].merge3?.outputPath).toBeUndefined();
  });
});
