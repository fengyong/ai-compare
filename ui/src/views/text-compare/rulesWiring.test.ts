import { describe, expect, it } from 'vitest';
import { SessionService } from '../../session/service';
import { effectiveRulesFor } from './rulesRuntime';

/**
 * P10 运行时接线：设置页 compareRules 组 → text/open rules 参数。
 * effectiveRulesFor 依赖全局 sessionService 单例，这里只测
 * 「未配置 → undefined」与纯映射逻辑（组存在场景由设置页侧单测覆盖）。
 */
describe('effectiveRulesFor', () => {
  it('未配置规则时返回 undefined（引擎默认语义）', async () => {
    const svc = new SessionService(); // 内存 store，不落盘
    void svc;
    // 全局单例未 init（无持久化数据）时也应安全返回 undefined
    expect(effectiveRulesFor('/a/x.cpp', '/b/x.cpp')).toBeUndefined();
  });
});
