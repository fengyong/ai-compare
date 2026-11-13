import { describe, it, expect } from 'vitest';
import {
  LAYER_ORDER,
  SCOPE_LABELS,
  applyPatchToLayer,
  availableScopes,
  mergeTrees,
  resolveEffective,
  scopeToLayer,
} from './scopes';

describe('session/scopes', () => {
  const globalTree = { compare: { mtimeToleranceSec: 2, contentCompare: 'auto' } };
  const kindTree = { compare: { mtimeToleranceSec: 5 }, display: { showOrphans: true } };
  const sessionTree = { compare: { contentCompare: 'always' } };

  it('同键四层覆盖优先级：session > parentPair > parentKind > kind > global', () => {
    const eff = resolveEffective({
      global: { compare: { a: 'g', shared: 'g' } },
      kind: { compare: { shared: 'k' } },
      parentKind: { compare: { shared: 'pk' } },
      parentPair: { compare: { shared: 'pp' } },
      session: { compare: { shared: 's' } },
    });
    expect(eff.values.compare.shared).toBe('s');
    expect(eff.origin.compare).toBe('session');

    // 逐层降级：去掉 session → parentPair；再去掉 parentPair → parentKind …
    const eff2 = resolveEffective({
      global: { compare: { shared: 'g' } },
      kind: { compare: { shared: 'k' } },
      parentKind: { compare: { shared: 'pk' } },
    });
    expect(eff2.values.compare.shared).toBe('pk');
    const eff3 = resolveEffective({ global: { compare: { shared: 'g' } }, kind: { compare: { shared: 'k' } } });
    expect(eff3.values.compare.shared).toBe('k');
    const eff4 = resolveEffective({ global: { compare: { shared: 'g' } } });
    expect(eff4.values.compare.shared).toBe('g');
  });

  it('合并保留低层键：高层只覆盖同名键', () => {
    const eff = resolveEffective({ global: globalTree, kind: kindTree, session: sessionTree });
    expect(eff.values.compare).toEqual({ mtimeToleranceSec: 5, contentCompare: 'always' });
    expect(eff.values.display).toEqual({ showOrphans: true });
    expect(eff.origin.compare).toBe('session');
    expect(eff.origin.display).toBe('kind');
  });

  it('空层/undefined 层安全跳过', () => {
    const eff = resolveEffective({ global: null, kind: undefined, session: sessionTree });
    expect(eff.values).toEqual(sessionTree);
    expect(eff.origin.compare).toBe('session');
  });

  it('mergeTrees：组级浅拷贝不污染入参', () => {
    const base = { compare: { a: 1 } };
    const out = mergeTrees(base, { compare: { b: 2 } });
    expect(out.compare).toEqual({ a: 1, b: 2 });
    expect(base.compare).toEqual({ a: 1 });
  });

  it('applyPatchToLayer：写回合并；undefined 删除该键覆盖', () => {
    const layer = { compare: { mtimeToleranceSec: 2, contentCompare: 'auto' } };
    const next = applyPatchToLayer(layer, { compare: { mtimeToleranceSec: 9 } });
    expect(next.compare).toEqual({ mtimeToleranceSec: 9, contentCompare: 'auto' });
    expect(layer.compare.mtimeToleranceSec).toBe(2); // 入参不变

    const removed = applyPatchToLayer(next, { compare: { contentCompare: undefined } });
    expect(removed.compare).toEqual({ mtimeToleranceSec: 9 });

    // 空层写入
    expect(applyPatchToLayer(null, { sync: { method: 'mirror' } }).sync).toEqual({ method: 'mirror' });
  });

  it('availableScopes：无父会话隐藏子会话专属两项', () => {
    const all = availableScopes(true);
    expect(all).toEqual(['session', 'parent-pair', 'parent-kind', 'kind', 'global']);
    const noParent = availableScopes(false);
    expect(noParent).toEqual(['session', 'kind', 'global']);
    for (const s of all) expect(SCOPE_LABELS[s]).toBeTruthy();
  });

  it('scopeToLayer 映射齐全（LAYER_ORDER 拉平顺序低→高）', () => {
    expect(LAYER_ORDER).toEqual(['global', 'kind', 'parentKind', 'parentPair', 'session']);
    expect(scopeToLayer('kind')).toBe('kind');
    expect(scopeToLayer('global')).toBe('global');
    expect(scopeToLayer('session')).toBe('session-write');
    expect(scopeToLayer('parent-pair')).toBe('parentPair');
    expect(scopeToLayer('parent-kind')).toBe('parentKind');
  });
});
