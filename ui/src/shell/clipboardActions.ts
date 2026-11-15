import { clipboardStore, latestTwo } from '../lib/clipboardStore';
import { S } from '../strings';
import { useShell } from './store';
import type { ClipboardGateway } from './viewRegistry';

type Notice = (msg: string) => void;

function messageOf(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

/** FR-CLIP-001：把系统剪贴板当前文本记入历史（权限不可用时静默沿用已有历史） */
export async function captureClipboard(clipboard: ClipboardGateway): Promise<void> {
  try {
    const text = await clipboard.readText();
    if (text) clipboardStore.add(text);
  } catch {
    // 浏览器剪贴板权限被拒：历史面板与已有条目仍可用
  }
}

/** FR-CLIP-002：Ctrl+Alt+C 比较历史中最近两条（较旧在左、最新在右） */
export async function compareLatestClipboard(notice: Notice): Promise<void> {
  const pair = latestTwo(clipboardStore.list());
  if (!pair) {
    notice(S.clip.needTwo);
    return;
  }
  try {
    const [left, right] = await Promise.all([
      clipboardStore.materialize(`clipboard://${pair[0].id}`),
      clipboardStore.materialize(`clipboard://${pair[1].id}`),
    ]);
    await useShell.getState().openTextTab(left, right);
    notice(S.clip.compared);
  } catch (err) {
    notice(`${S.clip.openFailed}: ${messageOf(err)}`);
  }
}

/** FR-CLIP-001 面板：指定条目与相邻的新一条比较（单条版快捷比较） */
export async function compareClipboardEntry(id: number, notice: Notice): Promise<void> {
  const entries = clipboardStore.list();
  const idx = entries.findIndex((x) => x.id === id);
  if (idx < 0) {
    notice(S.clip.missing);
    return;
  }
  const newer = entries[idx + 1];
  const older = entries[idx - 1];
  const other = newer ?? older;
  if (!other) {
    notice(S.clip.needTwo);
    return;
  }
  const [left, right] = newer ? [entries[idx], other] : [other, entries[idx]];
  try {
    const l = await clipboardStore.materialize(`clipboard://${left.id}`);
    const r = await clipboardStore.materialize(`clipboard://${right.id}`);
    await useShell.getState().openTextTab(l, r);
    notice(S.clip.compared);
  } catch (err) {
    notice(`${S.clip.openFailed}: ${messageOf(err)}`);
  }
}

/** FR-CLIP-001 面板「用文本编辑打开」：同一条目两侧打开文本比较（可编辑窗格） */
export async function openClipboardAsText(id: number, notice: Notice): Promise<void> {
  try {
    const path = await clipboardStore.materialize(`clipboard://${id}`);
    await useShell.getState().openTextTab(path, path);
  } catch (err) {
    notice(`${S.clip.openFailed}: ${messageOf(err)}`);
  }
}

/** FR-CLIP-004：文本比较会话「与剪贴板比较」，左侧为当前会话左文件，右侧为剪贴板文本 */
export async function comparePathWithClipboard(
  leftPath: string,
  clipboard: ClipboardGateway,
  notice: Notice,
): Promise<void> {
  let text = '';
  try {
    text = await clipboard.readText();
  } catch {
    // 权限失败时沿用历史最新一条
    text = clipboardStore.list()[clipboardStore.list().length - 1]?.text ?? '';
  }
  if (!text) {
    notice(S.clip.empty);
    return;
  }
  try {
    const spec = clipboardStore.registerText(text);
    const right = await clipboardStore.materialize(spec);
    await useShell.getState().openTextTab(leftPath, right);
    notice(S.clip.compared);
  } catch (err) {
    notice(`${S.clip.openFailed}: ${messageOf(err)}`);
  }
}
