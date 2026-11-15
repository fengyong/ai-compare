import { useState } from 'react';
import { useShell } from './store';
import { S } from '../strings';

/**
 * 壳级对话框集：关闭确认（SES-001 保存提示）/ 另存为 / 通用输入（重命名）。
 * 全部由 store 状态驱动（closeGuard / saveAsPrompt / prompt），AppShell 统一挂载。
 */

export function CloseGuardDialog() {
  const closeGuard = useShell((st) => st.closeGuard);
  const resolveCloseGuard = useShell((st) => st.resolveCloseGuard);
  const findTab = useShell((st) => st.findTab);
  if (!closeGuard) return null;
  const tab = findTab(closeGuard.tabId)?.tab;
  const content = closeGuard.kind === 'content';
  const title = content ? S.session.closeGuardContentTitle : S.session.closeGuardTitle;
  const dirtyFiles =
    content && tab?.text
      ? [
          tab.contentDirty?.left ? tab.text.left.name : null,
          tab.contentDirty?.right ? tab.text.right.name : null,
        ].filter((n): n is string => n !== null)
      : [];
  return (
    <div className="aic-modal-mask">
      <div className="aic-modal aic-dialog" role="alertdialog" aria-label={title}>
        <b>{title}</b>
        {content ? (
          <>
            <p>{S.session.closeGuardContentBody}</p>
            <ul className="aic-dialog-list">
              {dirtyFiles.map((n) => (
                <li key={n}>{n}</li>
              ))}
            </ul>
          </>
        ) : (
          <p>
            {S.session.closeGuardBody}
            {tab?.sessionLink && <>（{tab.sessionLink.name}）</>}
          </p>
        )}
        <div className="aic-dialog-actions">
          <button className="primary" onClick={() => void resolveCloseGuard('save')}>
            {S.session.saveAndClose}
          </button>
          <button onClick={() => void resolveCloseGuard('discard')}>
            {S.session.discardClose}
          </button>
          <button onClick={() => void resolveCloseGuard('cancel')}>{S.session.cancel}</button>
        </div>
      </div>
    </div>
  );
}

export function SaveAsDialog() {
  const saveAsPrompt = useShell((st) => st.saveAsPrompt);
  const resolveSaveAs = useShell((st) => st.resolveSaveAs);
  const [name, setName] = useState('');
  if (!saveAsPrompt) return null;
  const submit = () => void resolveSaveAs(name);
  return (
    <div className="aic-modal-mask">
      <div className="aic-modal aic-dialog" role="dialog" aria-label={S.session.saveAsTitle}>
        <b>{S.session.saveAsTitle}</b>
        <div className="aic-dialog-row">
          <label>{S.session.saveAsName}</label>
          <input
            autoFocus
            value={name}
            onChange={(e) => setName(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') submit();
              if (e.key === 'Escape') void resolveSaveAs(null);
            }}
          />
        </div>
        <div className="aic-dialog-actions">
          <button className="primary" disabled={!name.trim()} onClick={submit}>
            {S.session.saveAsOk}
          </button>
          <button onClick={() => void resolveSaveAs(null)}>{S.session.cancel}</button>
        </div>
      </div>
    </div>
  );
}

/** 通用确认框（R3-20 覆盖警示）：由 askConfirm 的 Promise 驱动，是/否都结清 */
export function ConfirmDialog() {
  const confirm = useShell((st) => st.confirm);
  const resolveConfirm = useShell((st) => st.resolveConfirm);
  if (!confirm) return null;
  return (
    <div className="aic-modal-mask">
      <div className="aic-modal aic-dialog" role="alertdialog" aria-label={confirm.title}>
        <b>{confirm.title}</b>
        <p>{confirm.message}</p>
        <div className="aic-dialog-actions">
          <button className="primary" onClick={() => resolveConfirm(true)}>
            {confirm.confirmLabel ?? S.session.ok}
          </button>
          <button onClick={() => resolveConfirm(false)}>{S.session.cancel}</button>
        </div>
      </div>
    </div>
  );
}

export function PromptDialog() {
  const prompt = useShell((st) => st.prompt);
  const resolvePrompt = useShell((st) => st.resolvePrompt);
  const [value, setValue] = useState<string | undefined>(undefined);
  if (!prompt) return null;
  const current = value ?? prompt.initialValue ?? '';
  const submit = () => resolvePrompt(current);
  return (
    <div className="aic-modal-mask">
      <div className="aic-modal aic-dialog" role="dialog" aria-label={prompt.title}>
        <b>{prompt.title}</b>
        <div className="aic-dialog-row">
          <input
            autoFocus
            value={current}
            onChange={(e) => setValue(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') submit();
              if (e.key === 'Escape') resolvePrompt(null);
            }}
          />
        </div>
        <div className="aic-dialog-actions">
          <button className="primary" onClick={submit}>
            确定
          </button>
          <button onClick={() => resolvePrompt(null)}>{S.session.cancel}</button>
        </div>
      </div>
    </div>
  );
}
