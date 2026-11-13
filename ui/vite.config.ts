import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

// Tauri 前端构建配置：
// - 在 Tauri CLI 环境下固定渲染目标（WebKitGTK / WKWebView 基线），浏览器 dev 时用默认 modules
// - dev server 固定端口，与 src-tauri/tauri.conf.json 的 devUrl 保持一致
const host = process.env.TAURI_DEV_HOST;

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: 'ws',
          host,
          port: 5174,
        }
      : undefined,
    watch: {
      ignored: ['**/src-tauri/**'],
    },
  },
  build: {
    // Tauri 2 支持的 WebView 基线（Chrome 105+ / Safari 13+）
    target: process.env.TAURI_ENV_PLATFORM == 'windows' ? 'chrome105' : 'safari13',
    minify: !process.env.TAURI_ENV_DEBUG ? 'esbuild' : false,
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
    chunkSizeWarningLimit: 1500,
  },
});
