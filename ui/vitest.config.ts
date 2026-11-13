import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    environment: 'node',
    include: ['src/**/*.test.ts', 'tests/**/*.test.ts'],
    // CM6 / React 相关模块不在单测中引入，保持纯逻辑测试的轻量
    globals: false,
  },
});
