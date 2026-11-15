import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    environment: 'node',
    // `.dom.test.tsx` = 组件级测试（jsdom 环境由文件头 `@vitest-environment` 声明）
    include: ['src/**/*.test.ts', 'src/**/*.test.tsx', 'tests/**/*.test.ts'],
    // 模块转换缓存放到 ui/ 下（不进 node_modules）：依赖变动时 vitest 会整体
    // 重建该目录，node_modules 内的大目录重建容易被工具链的批量删除护栏拦下。
    fsModuleCachePath: '.vitest-fs-cache',
    // CM6 / React 相关模块不在单测中引入，保持纯逻辑测试的轻量
    globals: false,
  },
});
