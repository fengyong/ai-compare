import { useEffect, useState } from 'react';
import { clipLabel, clipboardStore, latestTwo } from '../../lib/clipboardStore';
import {
  compareClipboardEntry,
  compareLatestClipboard,
  openClipboardAsText,
} from '../../shell/clipboardActions';
import { useShell } from '../../shell/store';
import { S } from '../../strings';

/**
 * FR-CLIP-001 剪贴板历史面板（BC 托盘图标单击后的最近条目列表等价物）。
 * 右键菜单在 WebView 中以行内按钮呈现：另存为… / 删除 / 与相邻条目比较 /
 * 用文本编辑打开。从列表删除条目不影响系统剪贴板。
 */
export function ClipboardHistoryPanel() {
  const open = useShell((st) => st.clipboardPanelOpen);
  const toggle = useShell((st) => st.toggleClipboardPanel);
  const notice = useShell((st) => st.setNotice);
  const [, force] = useState(0);

  useEffect(() => clipboardStore.subscribe(() => force((n) => n + 1)), []);

  if (!open) return null;
  const entries = [...clipboardStore.list()].reverse();
  const pair = latestTwo(clipboardStore.list());

  const capture = async () => {
    try {
      const text = await navigator.clipboard.readText();
      if (!text) {
        notice(S.clip.empty);
        return;
      }
      clipboardStore.add(text);
      notice(S.clip.captured);
    } catch {
      notice(S.clip.readDenied);
    }
  };

  return (
    <div className="aic-clip-backdrop" onClick={() => toggle(false)}>
      <div className="aic-clip-panel" onClick={(e) => e.stopPropagation()}>
        <div className="aic-clip-head">
          <span>{S.clip.title}</span>
          <span className="aic-clip-count">
            {entries.length} {S.clip.count}
          </span>
          <button onClick={() => toggle(false)}>{S.clip.close}</button>
        </div>
        <div className="aic-clip-toolbar">
          <button onClick={() => void capture()}>{S.clip.capture}</button>
          <button disabled={!pair} onClick={() => void compareLatestClipboard(notice)}>
            {S.clip.compare}
          </button>
          <button
            disabled={entries.length === 0}
            onClick={() => {
              clipboardStore.clear();
              notice(S.clip.cleared);
            }}
          >
            {S.clip.clear}
          </button>
          <span className="aic-clip-hint">{S.clip.byteHint}</span>
        </div>
        <div className="aic-clip-list">
          {entries.length === 0 && <div className="aic-clip-empty">{S.clip.emptyHint}</div>}
          {entries.map((entry) => (
            <div className="aic-clip-row" key={entry.id}>
              <span className="aic-clip-id">#{entry.id}</span>
              <span className="aic-clip-label" title={entry.text.slice(0, 200)}>
                {clipLabel(entry)}
              </span>
              <span className="aic-clip-time">{new Date(entry.ts).toLocaleString()}</span>
              <button onClick={() => void compareClipboardEntry(entry.id, notice)}>
                {S.clip.compareEntry}
              </button>
              <button onClick={() => void openClipboardAsText(entry.id, notice)}>
                {S.clip.openText}
              </button>
              <button onClick={() => clipboardStore.saveAs(entry.id)}>{S.clip.saveAs}</button>
              <button onClick={() => clipboardStore.remove(entry.id)}>{S.clip.delete}</button>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}
