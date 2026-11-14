import { isTauriRuntime } from '../lib/env';
import { useShell } from './store';

/**
 * 原生窗口关闭拦截（R3-05，FR-TXT-020 / FR-SES-001）。
 *
 * Tauri 的关闭请求默认直接销毁窗口，未落盘的编辑会随之丢失；这里一律
 * preventDefault，交给壳级关闭守卫逐个处理，全部处理完再 destroy()。
 * 浏览器演示模式没有原生关闭事件，不挂载（exitHandler 保持 null）。
 */
export function attachNativeExitGuard(): () => void {
  if (!isTauriRuntime()) return () => undefined;
  let disposed = false;
  let unlisten: (() => void) | null = null;

  const destroy = () => {
    void import('@tauri-apps/api/window').then(({ getCurrentWindow }) =>
      getCurrentWindow().destroy(),
    );
  };
  useShell.getState().setExitHandler(destroy);

  void import('@tauri-apps/api/window').then(async ({ getCurrentWindow }) => {
    const off = await getCurrentWindow().onCloseRequested((ev) => {
      ev.preventDefault();
      void useShell.getState().requestAppExit();
    });
    if (disposed) off();
    else unlisten = off;
  });

  return () => {
    disposed = true;
    unlisten?.();
    useShell.getState().setExitHandler(null);
  };
}
