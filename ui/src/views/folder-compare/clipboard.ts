/**
 * 剪贴板工具（P6b：FR-DIR-066 复制文件名/路径）。
 * 浏览器走 navigator.clipboard；Tauri WebView 中该 API 可能不可用/被拒，
 * 降级 document.execCommand('copy') 兜底，再失败则向上返回 'failed'
 * （视图层提示用户手动复制并把内容打印到控制台）。
 */
export type CopyOutcome = 'clipboard' | 'fallback' | 'failed';

export async function copyText(text: string): Promise<CopyOutcome> {
  if (text.length === 0) return 'failed';
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
      return 'clipboard';
    }
  } catch {
    // 落入 execCommand 兜底
  }
  try {
    const ta = document.createElement('textarea');
    ta.value = text;
    ta.style.position = 'fixed';
    ta.style.opacity = '0';
    document.body.appendChild(ta);
    ta.select();
    const ok = document.execCommand('copy');
    document.body.removeChild(ta);
    return ok ? 'fallback' : 'failed';
  } catch {
    return 'failed';
  }
}
