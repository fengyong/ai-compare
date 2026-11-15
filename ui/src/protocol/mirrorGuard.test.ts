/**
 * D1B 护栏：协议类型只能有一份定义——`generated/`（ts-rs 产物）。
 *
 * 背景：UI 曾长期维护「提案镜像」类型（`DirExecuteOp` / `DirExecuteParams` /
 * `DirExecuteResult` / `CharRange` / `Importance` 等），在引擎协议落地后两套定义
 * 开始各自演化，出现真实漂移：
 * - `detail` 被写成「必填且可为 null」，协议真相是「键可省略（`detail?: string`）」；
 * - `targetSide/targetPath/newName` 被写成 `| null`，协议真相是纯可选；
 * - `DirExecuteOpResult.status` 在镜像里对 succeeded 项缺省，协议里必填。
 * 结果：类型检查通过但运行时字段可能缺键，mock/真实引擎行为出现分叉。
 *
 * 本护栏禁止上述回潮：`generated/` 之外出现与生成类型同名的 `type`/`interface`
 * 声明即失败。需要本地别名时请**换个名字**（如 `type LocalPaneRange`），或在
 * 统一出口 `protocol/types.ts` 里 `export type { X }` 转发。
 *
 * 实现说明：用 `import.meta.glob(?raw)` 读源码而不是 `node:fs`——渲染层 tsconfig
 * 只声明 `types: ["vite/client"]`（无 Node 内置模块类型），且浏览器端代码
 * 本就不应依赖 Node 内置模块。
 */
import { describe, expect, it } from 'vitest';

/** 全部应用源码（排除生成目录），key 形如 `/src/views/x.tsx` */
const SOURCES = import.meta.glob(['/src/**/*.{ts,tsx}', '!/src/protocol/generated/**'], {
  query: '?raw',
  import: 'default',
  eager: true,
}) as Record<string, string>;

/** 生成类型：文件名即类型名（ts-rs 一类型一文件） */
const GENERATED = import.meta.glob('/src/protocol/generated/*.ts') as Record<string, unknown>;

/** `export type X = ...` / `interface X {` / `declare type X<T> = ...`（不含 re-export） */
const DECL_RE = /^[ \t]*(?:export[ \t]+)?(?:declare[ \t]+)?(?:type|interface)[ \t]+([A-Z]\w*)\b/gm;

/**
 * 去掉 import 语句（含多行 `import { type X } from '...'`）后再扫描：
 * 否则内联 `type X,` 形式的类型导入会被误判成本地声明。替换为等量换行以保住行号。
 */
function stripImports(text: string): string {
  return text.replace(/^[ \t]*import\s[\s\S]*?from\s+['"][^'"]*['"];?/gm, (m) =>
    '\n'.repeat(m.split('\n').length - 1),
  );
}

describe('protocol mirror guard（D1B：以生成类型为唯一来源）', () => {
  it('generated/ 之外不得重复声明同名协议类型', () => {
    const generated = new Set(
      Object.keys(GENERATED).map((p) =>
        p.replace('/src/protocol/generated/', '').replace(/\.ts$/, ''),
      ),
    );
    // 生成侧本身必须可读（目录/内容缺失时应立刻暴露，而不是静默通过）
    expect(generated.size).toBeGreaterThan(180);

    const offenders: string[] = [];
    for (const [path, raw] of Object.entries(SOURCES)) {
      const text = stripImports(raw);
      for (const m of text.matchAll(DECL_RE)) {
        const name = m[1];
        if (!generated.has(name)) continue;
        const line = text.slice(0, m.index ?? 0).split('\n').length;
        offenders.push(`${path.replace('/src/', 'src/')}:${line} 重复声明 ${name}`);
      }
    }

    expect(
      offenders,
      [
        '发现与协议生成类型同名的本地声明（D1B 禁止）：',
        ...offenders,
        '改为 `import type { X } from "../../protocol/types"`（需要对外暴露时加 `export type { X };`），',
        '本地确实需要不同语义时请换个名字，不要与协议类型同名。',
      ].join('\n'),
    ).toEqual([]);
  });
});
