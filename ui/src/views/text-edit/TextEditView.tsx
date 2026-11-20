import { useCallback, useEffect, useRef, useState } from 'react';
import { useShell } from '../../shell/store';
import type { Tab } from '../../shell/shellState';
import { S } from '../../strings';
import * as engine from '../../ipc/engineProcess';
import { detectLangId } from '../text-compare/langDetect';
import { loadLangExtension } from '../text-compare/langPacks';
import {
  compileQuery,
  expandReplacement,
  findMatches,
  replaceAllInLine,
  type CompiledQuery,
} from '../../lib/findCore';
import { EditablePane } from './editablePane';
import { buildDocText, parseDocText } from '../../lib/lineTable';
import {
  deleteEditDoc,
  docLinesOf,
  docNeedsSync,
  findDirtyEditDocByPath,
  getEditDoc,
  isDocDirty,
  linesEqual,
  newEditDoc,
  putEditDoc,
  syncDocToEngine,
  type EditDoc,
} from './editModel';
import { describeSaveError, saveEditDoc, savedNotice } from './textEditSave';
import { editorSyncIo } from './editIo';
import { EditFindBar, type EditFindState } from './EditFindBar';
import { registerEditView } from './editViewBus';

/**
 * 独立文本编辑视图（D2 / FR-EDIT-001 第一阶段）。
 *
 * 链路：`text/open`（省略 right，单文件会话）→ `text/lines`（左侧分页）→
 * CM6 单窗格编辑 → `text/edit` 增量收敛 → `text/save`（左侧，含另存为/备份）。
 * 引擎始终是内容与编码的权威：本视图不自行写盘、不改编码、不猜行尾。
 *
 * 与双栏比较视图的关系：**只复用文本能力与纯核心（text/lines、text/edit、
 * text/save、findCore、langPacks），不复用对齐窗格**——单文件编辑没有"另一侧"，
 * 用对齐模型冒充会产生填充行/差异段这类不存在的概念。
 */

const SYNC_DEBOUNCE_MS = 200;

interface FindHit {
  line: number;
  from: number;
  to: number;
}

/** 文档文本：已知 EOF 标记 → 精确还原；未知 → 退回 join（旧语义） */
function docTextOf(doc: EditDoc): string {
  return doc.trailingEol === null
    ? doc.lines.join('\n')
    : buildDocText(doc.lines, doc.trailingEol);
}

/** CM 文档文本 → 模型真值（行数组 + EOF 标记） */
function applyDocText(doc: EditDoc, text: string): void {
  if (doc.trailingEol === null) {
    doc.lines = docLinesOf(text, doc.lines);
    return;
  }
  const parsed = parseDocText(text);
  doc.lines = parsed.lines;
  doc.trailingEol = parsed.trailingEol;
}

/** 文档偏移：行数组 join('\n') 坐标系（与 CM 文档一致） */
function offsetOf(lines: readonly string[], line: number, col: number): number {
  let off = 0;
  for (let i = 0; i < line && i < lines.length; i++) off += lines[i].length + 1;
  return off + col;
}

export function TextEditView({ tab }: { tab: Tab }) {
  const edit = tab.edit;
  const setNotice = useShell((st) => st.setNotice);
  const markContentDirty = useShell((st) => st.markContentDirty);
  const retargetEditPath = useShell((st) => st.retargetEditPath);
  const openPrompt = useShell((st) => st.openPrompt);

  const hostRef = useRef<HTMLDivElement>(null);
  const paneRef = useRef<EditablePane | null>(null);
  const docRef = useRef<EditDoc | null>(null);
  const disposedRef = useRef(false);
  const queueRef = useRef<Promise<void>>(Promise.resolve());
  const debounceRef = useRef<number | null>(null);
  const lastNoticeRef = useRef<string>('');

  const [, bump] = useState(0);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [readOnly, setReadOnly] = useState(false);
  const [wrap, setWrap] = useState(true);
  const wrapRef = useRef(true);
  const [backup, setBackup] = useState(true);
  const [saving, setSaving] = useState(false);
  const [cursor, setCursor] = useState({ line: 0, col: 0 });
  const [findState, setFindState] = useState<EditFindState | null>(null);
  const [findResult, setFindResult] = useState<{
    total: number;
    index: number;
    error: string | null;
  }>({ total: -1, index: 0, error: null });
  const findHitsRef = useRef<FindHit[]>([]);
  const findIndexRef = useRef(0);

  const notify = useCallback(
    (msg: string) => {
      if (lastNoticeRef.current === msg) return;
      lastNoticeRef.current = msg;
      setNotice(msg);
    },
    [setNotice],
  );

  /** 串行化引擎同步（编辑落点 → text/edit；不并发，避免区间错位） */
  const enqueueSync = useCallback((): Promise<void> => {
    const next = queueRef.current.then(async () => {
      const doc = docRef.current;
      if (!doc || disposedRef.current) return;
      try {
        const outcome = await syncDocToEngine(doc, editorSyncIo(doc.sessionId));
        if (disposedRef.current) return;
        if (outcome === 'unsupported') {
          setReadOnly(true);
          notify(S.edit.editUnsupported);
        } else if (outcome === 'mismatch') {
          notify(S.edit.diverged);
        }
      } catch (err) {
        notify(describeSaveError(err));
      }
    });
    queueRef.current = next.catch(() => undefined);
    return next;
  }, [notify]);

  const scheduleSync = useCallback(() => {
    if (debounceRef.current !== null) window.clearTimeout(debounceRef.current);
    debounceRef.current = window.setTimeout(() => {
      debounceRef.current = null;
      void enqueueSync();
    }, SYNC_DEBOUNCE_MS);
  }, [enqueueSync]);

  /** 保存/关闭前把待发编辑全部交给引擎（含防抖中的那一批） */
  const flushSync = useCallback(async (): Promise<void> => {
    if (debounceRef.current !== null) {
      window.clearTimeout(debounceRef.current);
      debounceRef.current = null;
      await enqueueSync();
    } else {
      await queueRef.current;
    }
    const doc = docRef.current;
    if (doc && !linesEqual(doc.engineSnapshot, doc.lines)) await enqueueSync();
  }, [enqueueSync]);

  // ---- 打开（缓存水合 → 引擎打开 → 行内容分页） ----
  useEffect(() => {
    disposedRef.current = false;
    const sessionId = edit?.sessionId;
    const path = edit?.path;
    if (!sessionId || !path) return;

    const attach = (doc: EditDoc, editable: boolean): void => {
      docRef.current = doc;
      putEditDoc(doc);
      const host = hostRef.current;
      if (!host) return;
      const pane = EditablePane.create(host, {
        // A3：已知 EOF 标记时按它构建文档（末行之后是否还有一个空行），
        // 否则退回旧语义（join，不假设末尾换行）
        docText: docTextOf(doc),
        onChange: (text) => {
          const cur = docRef.current;
          if (!cur) return;
          applyDocText(cur, text);
          markContentDirty(tab.id, { left: isDocDirty(cur), right: false });
          bump((v) => v + 1);
          scheduleSync();
        },
        onCursor: (line, col) => setCursor({ line: line + 1, col: col + 1 }),
        readOnly: !editable,
        wrap,
      });
      paneRef.current = pane;
      setReadOnly(!editable);
      // 只读降级必须说明原因：用户否则会以为"能编辑但点不动"
      if (!editable) notify(S.edit.editUnsupported);
      void loadLangExtension(detectLangId(doc.name)).then((ext) => {
        if (!disposedRef.current && ext) pane.setLanguage(ext);
      });
      setLoading(false);
    };

    const cached = getEditDoc(sessionId);
    if (cached) {
      // 标签切换回来：直接水合（不重开会话，避免丢掉 UI 侧未落盘编辑）
      attach(cached, engine.getEngineCapabilities()?.textEdit === true && !cached.diverged);
      return () => {
        disposedRef.current = true;
        paneRef.current?.destroy();
        paneRef.current = null;
      };
    }

    const caps = engine.getEngineCapabilities();
    const editable = caps?.textEdit === true;
    void (async () => {
      try {
        const opened = await engine.openText({
          sessionId,
          left: path,
          // 新引擎：省略 right = 单文件会话；旧引擎（无 textOpenOne）退回同路径双开，
          // 行为等价但多一次读盘 + 一次 identical diff（兼容优先，不猜）
          ...(caps?.textOpenOne ? {} : { right: path }),
        });
        const engineLines = await engine.loadSideLines(
          sessionId,
          'left',
          opened.leftLineCount,
          path,
        );
        if (disposedRef.current) return;
        engine.trackSession(sessionId, 'text', {
          left: path,
          ...(caps?.textOpenOne ? {} : { right: path }),
        });
        // 崩溃重放/重新打开同一文件：旧（已死）会话里的未落盘编辑按路径认领
        // 回来，引擎真值（文件内容）作为收敛基准，随后重放用户输入。只认领
        // isSessionTracked 判为已死的会话，活标签的编辑不得抢（UI-01）。
        const stale = findDirtyEditDocByPath(path, sessionId, engine.isSessionTracked);
        // A3：旧引擎（无 textOpenEol 能力）不回传 → null = 退回旧语义
        const engineTrailing =
          typeof opened.leftTrailingEol === 'boolean' ? opened.leftTrailingEol : null;
        const doc = newEditDoc({
          sessionId,
          path,
          name: edit?.name || path.split(/[\\/]/).pop() || path,
          encoding: opened.leftEncoding,
          lines: stale ? [...stale.lines] : engineLines,
          // UI 真值优先认领旧文档（未落盘编辑含 EOF 改动）；引擎不回报则保持 null
          trailingEol:
            engineTrailing === null
              ? null
              : (stale?.trailingEol ?? engineTrailing),
        });
        if (stale) {
          doc.savedLines = [...stale.savedLines];
          doc.savedTrailingEol = stale.savedTrailingEol;
          deleteEditDoc(stale.sessionId);
        }
        doc.engineSnapshot = [...engineLines];
        doc.engineTrailingEol = engineTrailing;
        putEditDoc(doc);
        attach(doc, editable);
        if (isDocDirty(doc)) scheduleSync();
      } catch (err) {
        if (disposedRef.current) return;
        setError(`${S.edit.openFail}：${err instanceof Error ? err.message : String(err)}`);
        setLoading(false);
      }
    })();

    return () => {
      // 卸载（切标签/重绑）前把待发编辑交给引擎：否则引擎侧长期落后于 UI，
      // 期间任何"按引擎内容落盘"的路径都会写出旧内容。
      const doc = docRef.current;
      if (doc && docNeedsSync(doc)) {
        void syncDocToEngine(doc, editorSyncIo(doc.sessionId)).catch(() => undefined);
      }
      disposedRef.current = true;
      paneRef.current?.destroy();
      paneRef.current = null;
    };
    // 仅随会话句柄/路径变化重开；wrap 等交互状态不触发重开
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [edit?.sessionId, edit?.path, tab.id]);

  // 卸载时清掉防抖（引擎同步队列自行收敛）
  useEffect(() => {
    return () => {
      if (debounceRef.current !== null) window.clearTimeout(debounceRef.current);
    };
  }, []);

  const doSave = useCallback(
    async (target?: string): Promise<void> => {
      const doc = docRef.current;
      if (!doc) return;
      setSaving(true);
      try {
        await flushSync();
        const res = await saveEditDoc(doc, { backup, path: target });
        if (!res.ok) {
          // 失败不清脏、不改目标：提示原因，编辑内容仍在
          setNotice(res.message);
          return;
        }
        if (res.retargeted) retargetEditPath(tab.id, res.path);
        // A0：脏标记只由"文件是否等于 UI 最新内容"决定——保存期间的新编辑仍脏
        markContentDirty(tab.id, { left: isDocDirty(doc), right: false });
        setNotice(savedNotice(res));
        bump((v) => v + 1);
      } finally {
        setSaving(false);
      }
    },
    [backup, flushSync, markContentDirty, retargetEditPath, setNotice, tab.id],
  );

  const doSaveAs = useCallback((): void => {
    const doc = docRef.current;
    if (!doc) return;
    openPrompt({
      title: S.edit.saveAsTitle,
      initialValue: doc.path,
      onSubmit: (value) => {
        if (value === null) return;
        const path = value.trim();
        if (!path) return;
        void doSave(path);
      },
    });
  }, [doSave, openPrompt]);

  // ---- 查找 / 替换（lib/findCore 纯核心 + CM 事务） ----
  const compiledNow = useCallback((st: EditFindState): CompiledQuery => {
    return compileQuery({
      needle: st.needle,
      regex: st.regex,
      caseSensitive: st.caseSensitive,
      wholeWord: st.wholeWord,
    });
  }, []);

  const refreshFind = useCallback(
    (st: EditFindState): void => {
      const doc = docRef.current;
      if (!doc) return;
      const compiled = compiledNow(st);
      if (!compiled.ok) {
        findHitsRef.current = [];
        setFindResult({
          total: st.needle === '' ? -1 : 0,
          index: 0,
          error: compiled.error || null,
        });
        return;
      }
      const hits = findMatches(doc.lines, compiled);
      findHitsRef.current = hits;
      findIndexRef.current = Math.min(findIndexRef.current, Math.max(hits.length - 1, 0));
      setFindResult({ total: hits.length, index: findIndexRef.current, error: null });
    },
    [compiledNow],
  );

  const doFind = useCallback(
    (backward: boolean): void => {
      const doc = docRef.current;
      const pane = paneRef.current;
      if (!doc || !pane || !findState) return;
      refreshFind(findState);
      const hits = findHitsRef.current;
      if (hits.length === 0) return;
      const cur = pane.cursor();
      const curOff = offsetOf(doc.lines, cur.line, cur.col);
      let idx: number;
      if (backward) {
        idx = hits.length - 1;
        for (let i = hits.length - 1; i >= 0; i--) {
          if (offsetOf(doc.lines, hits[i].line, hits[i].from) < curOff) {
            idx = i;
            break;
          }
        }
      } else {
        idx = 0;
        for (let i = 0; i < hits.length; i++) {
          if (offsetOf(doc.lines, hits[i].line, hits[i].from) > curOff) {
            idx = i;
            break;
          }
        }
      }
      findIndexRef.current = idx;
      setFindResult({ total: hits.length, index: idx, error: null });
      const hit = hits[idx];
      const from = offsetOf(doc.lines, hit.line, hit.from);
      pane.selectRange(from, offsetOf(doc.lines, hit.line, hit.to));
    },
    [findState, refreshFind],
  );

  const replaceAt = useCallback(
    (hit: FindHit, st: EditFindState): void => {
      const doc = docRef.current;
      const pane = paneRef.current;
      if (!doc || !pane) return;
      const compiled = compiledNow(st);
      if (!compiled.ok) return;
      const lineText = doc.lines[hit.line] ?? '';
      const matched = lineText.slice(hit.from, hit.to);
      let groups: string[] = [];
      if (st.regex) {
        const re = new RegExp(compiled.re.source, compiled.re.flags.replace('g', ''));
        const m = re.exec(matched);
        groups = m ? m.slice(1).map((g) => g ?? '') : [];
      }
      const insert = expandReplacement(matched, st.replacement, st.regex, groups);
      pane.replaceRange(
        offsetOf(doc.lines, hit.line, hit.from),
        offsetOf(doc.lines, hit.line, hit.to),
        insert,
      );
    },
    [compiledNow],
  );

  const doReplaceCurrent = useCallback((): void => {
    if (!findState) return;
    const hits = findHitsRef.current;
    const idx = findIndexRef.current;
    if (hits.length === 0 || idx >= hits.length) return;
    replaceAt(hits[idx], findState);
    // 替换后行内容变化：按新内容重算命中，光标停在原位置附近
    window.setTimeout(() => refreshFind(findState), 0);
  }, [findState, refreshFind, replaceAt]);

  const doReplaceAll = useCallback((): void => {
    const doc = docRef.current;
    const pane = paneRef.current;
    if (!doc || !pane || !findState) return;
    const compiled = compiledNow(findState);
    if (!compiled.ok) return;
    const next = doc.lines.map(
      (line) => replaceAllInLine(line, compiled, findState.replacement, findState.regex) ?? line,
    );
    if (linesEqual(next, doc.lines)) return;
    // A3：整篇重写必须带上 EOF 标记，否则尾换行被 replaceRange 抹掉，
    // 保存时静默丢字节（与逐字编辑同一份 buildDocText 口径）
    const text =
      doc.trailingEol === null ? next.join('\n') : buildDocText(next, doc.trailingEol);
    pane.replaceRange(0, pane.view.state.doc.length, text);
    window.setTimeout(() => refreshFind(findState), 0);
  }, [compiledNow, findState, refreshFind]);

  /** 打开查找（命令 Ctrl+F 走这里；已打开则聚焦面板） */
  const openFind = useCallback((): void => {
    if (findState) return;
    const init: EditFindState = {
      needle: '',
      replacement: '',
      regex: false,
      caseSensitive: false,
      wholeWord: false,
    };
    findIndexRef.current = 0;
    setFindState(init);
    refreshFind(init);
  }, [findState, refreshFind]);

  const closeFind = useCallback((): void => {
    setFindState(null);
    setFindResult({ total: -1, index: 0, error: null });
    paneRef.current?.focus();
  }, []);

  const toggleWrap = useCallback((): void => {
    wrapRef.current = !wrapRef.current;
    setWrap(wrapRef.current);
    paneRef.current?.setWrap(wrapRef.current);
  }, []);

  // 壳级命令入口（命令表/快捷键经此调用；栈式注册，卸载即摘除）
  useEffect(() => {
    return registerEditView({
      openFind,
      toggleWrap,
      saveFile: () => void doSave(),
      saveFileAs: doSaveAs,
    });
  }, [openFind, toggleWrap, doSave, doSaveAs]);

  const onFindStateChange = useCallback(
    (next: EditFindState): void => {
      setFindState(next);
      findIndexRef.current = 0;
      refreshFind(next);
    },
    [refreshFind],
  );

  const doc = docRef.current;
  const dirty = doc ? isDocDirty(doc) : false;

  return (
    <div className="aic-edit-view">
      <div className="aic-edit-toolbar">
        <span className="aic-edit-name" title={doc?.path ?? edit?.path}>
          {doc?.name ?? edit?.name ?? ''}
        </span>
        {dirty && <span className="aic-edit-dirty" title={S.edit.dirtyDot} />}
        {readOnly && <span className="aic-edit-badge">{S.edit.readonlyBadge}</span>}
        {doc && <span className="aic-edit-badge">{`${S.edit.encodingLabel} ${doc.encoding}`}</span>}
        <span className="aic-edit-spacer" />
        <button onClick={() => void doSave()} disabled={saving || loading || readOnly}>
          {S.edit.saveOk}
        </button>
        <button onClick={doSaveAs} disabled={saving || loading || readOnly}>
          {S.text.saveFileAs}
        </button>
        <label className="aic-edit-check" title={S.edit.backupHint}>
          <input type="checkbox" checked={backup} onChange={(e) => setBackup(e.target.checked)} />
          {S.edit.backupToggle}
        </label>
        <button onClick={() => paneRef.current?.undo()} disabled={readOnly} title={S.edit.undo}>
          ↶
        </button>
        <button onClick={() => paneRef.current?.redo()} disabled={readOnly} title={S.edit.redo}>
          ↷
        </button>
        <label className="aic-edit-check">
          <input type="checkbox" checked={wrap} onChange={toggleWrap} />
          {S.edit.wrapToggle}
        </label>
        <button onClick={() => (findState ? closeFind() : openFind())}>{S.edit.find}</button>
      </div>

      {findState && (
        <EditFindBar
          state={findState}
          onChange={onFindStateChange}
          result={findResult}
          onFind={doFind}
          onReplaceCurrent={doReplaceCurrent}
          onReplaceAll={doReplaceAll}
          onClose={closeFind}
        />
      )}

      {loading && <div className="aic-edit-loading">{S.edit.loading}</div>}
      {error && <div className="aic-edit-error">{error}</div>}

      <div className="aic-edit-host" ref={hostRef} />

      <div className="aic-edit-status">
        {doc ? (
          <>
            <span>{S.edit.lineCount(doc.lines.length)}</span>
            <span>{S.edit.cursorAt(cursor.line, cursor.col)}</span>
            <span>{`${S.edit.encodingLabel} ${doc.encoding}`}</span>
            <span className="dim">{S.edit.eolPreserved}</span>
            {/* A3：末行行尾符是字节级信息（"a\n" ≠ "a"），状态栏显式给出 */}
            {doc.trailingEol !== null && (
              <span className="dim" title={S.edit.eolPreserved}>
                {doc.trailingEol ? S.edit.eofOn : S.edit.eofOff}
              </span>
            )}
            {doc.diverged && <span className="aic-edit-error-inline">{S.edit.diverged}</span>}
          </>
        ) : (
          <span className="dim">{edit?.path ?? ''}</span>
        )}
      </div>
    </div>
  );
}

/** 关闭标签时释放会话级模型（由 cleanupTabEngines 调用） */
export function releaseEditDoc(sessionId: string): void {
  deleteEditDoc(sessionId);
}
