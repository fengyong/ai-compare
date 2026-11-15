import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { EditorSelection, type Extension } from '@codemirror/state';
import type { Tab } from '../../shell/store';
import {
  closeSession,
  editText,
  filterDir,
  getEngineCapabilities,
  openText,
  requestInlineDiff,
  saveText,
  trackSession,
} from '../../ipc/engineProcess';
import { useShell } from '../../shell/store';
import { RpcError } from '../../protocol/client';
import { S } from '../../strings';
import {
  ENCODING_CHOICES,
  encodingById,
  encodingDisplayName,
  encodingParam,
  type EncodingId,
} from '../../lib/encodings';

import {
  compileQuery,
  expandReplacement,
  findMatches,
  findNext,
  matchedLineNos,
  replaceAllInLine,
  type CompiledQuery,
} from '../../lib/findCore';
import { diffLinesOps, summarizeSections, toSections } from '../../lib/lineDiffCore';
import { buildInlineMarks, mergeDmpMarks, type InlineMark } from '../../lib/inlineDiffCore';
import {
  buildAlignedDocs,
  findDiffSectionAtLine,
  type AlignedDoc,
  type AlignedRow,
} from './alignment';
import { loadTextSession, loadTextSessionReuse } from './textSession';
import { effectiveRulesFor } from './rulesRuntime';
import type { Importance, Section, TextEditOp } from '../../protocol/types';
import {
  allCollapsibleKeys,
  computeFolds,
  defaultCollapsedKeys,
  hiddenRowCount,
  unfoldKeysFor,
  type FoldRange,
} from './folding';
import {
  applySidePatches,
  opsFromPatches,
  patchSideFromDocEdit,
  patchesFromLineReplacement,
  type SidePatch,
} from './editPatch';
import { flushEdits, newEditSyncState, type EditSyncState } from './editSync';
import {
  emptyUndoState,
  popRedo,
  popUndo,
  pushUndo,
  redoPatches,
  tryCoalesce,
  undoPatches,
  type UndoEntry,
} from './undoStack';
import {
  getTextSession,
  isTextModified,
  newTextSessionModel,
  putTextSession,
  type TextSessionModel,
} from './textModel';
import { applySavedEntries, runTextSave, saveRequestsFor, type SaveRequest } from './textSave';
import { parentRescanParams } from './parentWriteback';
import { AlignedPane, type AlignedPaneHandle, type PaneContentInput } from './AlignedPane';
import { FindPanel, type FindPanelState } from './FindPanel';
import { OverviewBar } from './OverviewBar';
import { DiffListPanel } from './DiffListPanel';
import { detectLangId } from './langDetect';
import { loadLangExtension } from './langPacks';
import {
  BOOKMARK_SLOTS,
  bookmarkLabel,
  clearBookmarks,
  nextBookmarkLine,
  prevBookmarkLine,
  setBookmark as setBookmarkSlot,
  toggleBookmark,
  type BookmarkMap,
} from './bookmarks';
import { registerTextView } from './textViewBus';

type FindMatch = { pane: 'left' | 'right'; line: number; from: number; to: number };
type FindInfo = { matches: FindMatch[]; compiled: CompiledQuery; error: string | null };

/**
 * 文本比较视图（docs/03 §3.4.3 M32 的 P5 交付：FR-TXT-001~004/013/014/020~023/030、
 * FIND-001、UNDO-001、ENC-001）：
 * - CM6 双窗格可编辑（对齐文档 1:1 行 + 占位填充行），编辑实时重 diff
 *   （R3-18：引擎 text/edit 权威重算，保留重要性分级；无能力位时回落
 *   客户端全量重算——lib/lineDiffCore）；
 * - 会话级撤销/重做（UNDO-001，跨窗格同栈，独立于 CM history）；
 * - 等值区折叠（上下文 N 行可配）+ 全部展开/折叠 + 跨折叠差异导航 +
 *   差异列表当前位置同步高亮（FR-TXT-020~023/030）；
 * - 查找替换面板（正则/大小写/整词/侧选择/匹配计数/替换/全选匹配行，FIND-001）；
 * - 概览条（mini-map 色带 + 视口矩形 + 点击跳转 + 滚动同步，FR-TXT-030）；
 * - 语法高亮（扩展名 → Lezer 语言包按需加载，无包回退纯文本）；
 * - 编码切换（会话级，重载重 diff，BOM 与引擎对齐——lib/encodings）。
 *
 * 会话数据（编辑内容/撤销栈/编码/折叠状态）存活于 textModel.ts 缓存：
 * 标签页切换不丢编辑（重新挂载时缓存水合）。
 */
export function TextCompareView({ tab }: { tab: Tab }) {
  const text = tab.text!;
  const [error, setError] = useState<string | null>(null);
  const [ready, setReady] = useState(false);
  /** UI 渲染版本号（model 存 ref，重 diff 后自增触发渲染） */
  const [uiVersion, setUiVersion] = useState(0);
  const [navIndex, setNavIndex] = useState(-1);
  const [currentSectionId, setCurrentSectionId] = useState<number | null>(null);
  const [viewport, setViewport] = useState<{ topRow: number; bottomRow: number } | null>(null);
  const [showDiffList, setShowDiffList] = useState(false);
  const [editing, setEditing] = useState(true);
  const [syntaxOn, setSyntaxOn] = useState(true);
  const [contextRows, setContextRows] = useState(3);
  const [findOpen, setFindOpen] = useState(false);
  const [findState, setFindState] = useState<FindPanelState>({
    needle: '',
    replacement: '',
    regex: false,
    caseSensitive: false,
    wholeWord: false,
    side: 'left',
  });
  const [findIdx, setFindIdx] = useState(0);
  /** FR-TXT-002 自动换行（工具栏「换行」按钮 / 视图菜单） */
  const [wrap, setWrap] = useState(false);
  /** FR-TXT-023 书签：编号 0-9 → 0 基对齐行号 */
  const [bookmarks, setBookmarks] = useState<BookmarkMap>({});

  // ------------------------------------------------------------------
  // 非响应式会话数据（textModel 缓存 + refs）
  // ------------------------------------------------------------------
  const modelRef = useRef<TextSessionModel | null>(null);
  /** 视图当前绑定的会话：差异文件跳转原地换会话，旧会话的在途重建不得回灌（FR-DIR-071） */
  const liveSessionRef = useRef<string | null>(null);
  const derivedRef = useRef<{
    left: AlignedDoc;
    right: AlignedDoc;
    rows: AlignedRow[];
    marks: { left: InlineMark[]; right: InlineMark[] };
    importance: Map<number, Importance>;
    diffSections: Section[];
    folds: FoldRange[];
  } | null>(null);
  const collapsedRef = useRef<Set<number>>(new Set());
  const currentSectionIdRef = useRef<number | null>(null);
  const paneL = useRef<AlignedPaneHandle | null>(null);
  const paneR = useRef<AlignedPaneHandle | null>(null);
  const syncingRef = useRef(false);
  const rafRef = useRef<number | null>(null);
  const editingRef = useRef(true);
  editingRef.current = editing;
  const syntaxOnRef = useRef(true);
  syntaxOnRef.current = syntaxOn;
  const contextRowsRef = useRef(3);
  contextRowsRef.current = contextRows;
  const findStateRef = useRef(findState);
  findStateRef.current = findState;
  const findIdxRef = useRef(0);
  findIdxRef.current = findIdx;
  const wrapRef = useRef(false);
  wrapRef.current = wrap;
  const bookmarksRef = useRef<BookmarkMap>({});
  bookmarksRef.current = bookmarks;
  /** 最近一次语法高亮扩展（refreshDecorations 复用） */
  const langExtRef = useRef<Extension | null>(null);
  /** 窗格 handle 首次双齐后的补同步标记（初始 rebuild 早于窗格挂载，setContent 会落空） */
  const handlesSyncedRef = useRef(false);

  const paneHandles = () => ({ left: paneL.current, right: paneR.current });

  const syncCurrentSection = useCallback((id: number | null) => {
    currentSectionIdRef.current = id;
    setCurrentSectionId(id);
  }, []);

  // ---- 派生数据（对齐文档/折叠/重要性）----
  const computeDerived = useCallback((model: TextSessionModel) => {
    const aligned = buildAlignedDocs(model.sections, model.leftLines, model.rightLines);
    const importance = new Map<number, Importance>();
    for (const s of model.sections) importance.set(s.id, s.important);
    const diffSections = model.sections.filter((s) => s.kind !== 'equal');
    const folds = computeFolds(aligned.left.rows, contextRowsRef.current, collapsedRef.current);
    return { ...aligned, rows: aligned.left.rows, importance, diffSections, folds };
  }, []);

  // ---- 行内差异：text/inlineDiff 按 section 懒计算（P3 生成协议；能力位）→ 客户端词级兜底 ----
  const computeInlineMarks = useCallback(
    async (model: TextSessionModel): Promise<{ left: InlineMark[]; right: InlineMark[] }> => {
      const out: { left: InlineMark[]; right: InlineMark[] } = { left: [], right: [] };
      const targets = model.sections.filter(
        (s) =>
          s.kind === 'change' &&
          s.left !== undefined &&
          s.right !== undefined &&
          s.left.end - s.left.start <= 400,
      );
      // 每个替换段一次懒计算 RPC（mock 同步、真实引擎缓存在会话侧）
      const results = await Promise.all(
        targets.map((s) =>
          requestInlineDiff({ sessionId: model.sessionId, sectionId: s.id }).then((res) => ({
            section: s,
            res,
          })),
        ),
      );
      for (const { section, res } of results) {
        if (res && res.lines.length > 0) {
          for (const ld of res.lines) {
            for (const span of ld.spans) {
              if (span.kind === 'equal') continue;
              if (span.left && span.left.end > span.left.start) {
                out.left.push({ line: ld.leftLine, from: span.left.start, to: span.left.end });
              }
              if (span.right && span.right.end > span.right.start) {
                out.right.push({ line: ld.rightLine, from: span.right.start, to: span.right.end });
              }
            }
          }
          continue;
        }
        // 客户端兜底（wordDiff 词级 LCS；RPC 失败/能力位关闭的 section）
        const s = section;
        const lineCount = Math.min(s.left!.end - s.left!.start, s.right!.end - s.right!.start);
        for (let k = 0; k < lineCount; k++) {
          const li = s.left!.start + k;
          const ri = s.right!.start + k;
          const l = model.leftLines[li] ?? '';
          const r = model.rightLines[ri] ?? '';
          if (l.length > 1000 || r.length > 1000) continue;
          const pairs = buildInlineMarks(
            [l],
            [r],
            0,
            0,
            1,
            model.dmpCleanup ? { dmpCleanup: true } : {},
          );
          for (const m of pairs.left) out.left.push({ line: li, from: m.from, to: m.to });
          for (const m of pairs.right) out.right.push({ line: ri, from: m.from, to: m.to });
        }
      }
      // DMP 清理（UI 后处理开关）：对 RPC 来源的 spans 同样生效（引擎默认 None）
      if (model.dmpCleanup) {
        out.left = mergeDmpMarks(out.left);
        out.right = mergeDmpMarks(out.right);
      }
      return out;
    },
    [],
  );

  // ---- 查找匹配（对齐文档行坐标；填充行文本为空，天然无匹配） ----
  const computeFind = useCallback((): FindInfo => {
    const q = findStateRef.current;
    const compiled = compileQuery(q);
    if (!compiled.ok) {
      return { matches: [], compiled, error: compiled.error === '' ? null : compiled.error };
    }
    const derived = derivedRef.current;
    const matches: FindMatch[] = [];
    if (derived) {
      const panes: Array<'left' | 'right'> = q.side === 'both' ? ['left', 'right'] : [q.side];
      for (const pane of panes) {
        for (const m of findMatches(derived[pane].lines, compiled)) {
          matches.push({ pane, ...m });
        }
      }
    }
    return { matches, compiled, error: null };
  }, []);

  // ---- 重 diff + 回灌（编辑/撤销/折叠/查找/编码统一入口）----
  const rebuildAndRender = useCallback(
    async (model: TextSessionModel) => {
      const derived = computeDerived(model);
      const marks = await computeInlineMarks(model);
      const langId = syntaxOnRef.current ? detectLangId(model.leftName) : null;
      const ext = await loadLangExtension(langId);
      // 在途重建可能属于已被跳转替换掉的旧会话：提交前核对，否则新文件显示旧文件内容
      if (liveSessionRef.current !== model.sessionId) return;
      langExtRef.current = ext;
      derivedRef.current = { ...derived, marks };
      // 折叠状态持久化（会话级，标签切换恢复）
      model.collapsedKeys = [...collapsedRef.current];
      model.contextRows = contextRowsRef.current;
      putTextSession(model);
      modelRef.current = model;

      const { left: hL, right: hR } = paneHandles();
      const findInfo = computeFind();
      if (findIdxRef.current >= findInfo.matches.length) {
        findIdxRef.current = 0;
        setFindIdx(0);
      }
      const current = findInfo.matches[findIdxRef.current] ?? null;
      const buildInput = (pane: 'left' | 'right'): PaneContentInput => ({
        docText: derived[pane].lines.join('\n'),
        rows: derived.rows,
        sectionImportance: derived.importance,
        marks: marks[pane],
        folds: derived.folds,
        findMarks: findInfo.matches
          .filter((m) => m.pane === pane)
          .map((m) => ({ line: m.line, from: m.from, to: m.to, current: m === current })),
        currentSectionId:
          currentSectionIdRef.current !== null &&
          derived.diffSections.some((s) => s.id === currentSectionIdRef.current)
            ? currentSectionIdRef.current
            : null,
        langExtension: ext,
        editable: editingRef.current,
        bookmarks: bookmarksRef.current,
      });
      hL?.setContent(buildInput('left'));
      hR?.setContent(buildInput('right'));

      // 越界的导航状态清理
      if (
        currentSectionIdRef.current !== null &&
        !derived.diffSections.some((s) => s.id === currentSectionIdRef.current)
      ) {
        syncCurrentSection(null);
        setNavIndex(-1);
      }
      setUiVersion((v) => v + 1);
    },
    [computeDerived, computeInlineMarks, computeFind, syncCurrentSection],
  );

  /**
   * 窗格 handle 注册回调：双齐且会话数据已就绪时补一次重建。
   * 修复"初始 rebuild 早于窗格挂载 → setContent 落空 → 窗格空白"：
   * panes 由 ready 门控渲染，首个 rebuild 必然跑在窗格挂载之前，
   * 句柄就绪（含 StrictMode 重挂载/标签切换回来）后需重灌内容。
   */
  const registerPaneHandle = useCallback(
    (slot: 'left' | 'right') => (h: AlignedPaneHandle | null) => {
      if (slot === 'left') paneL.current = h;
      else paneR.current = h;
      if (h === null) {
        handlesSyncedRef.current = false;
        return;
      }
      if (!handlesSyncedRef.current && paneL.current && paneR.current && modelRef.current) {
        handlesSyncedRef.current = true;
        void rebuildAndRender(modelRef.current);
      }
    },
    [rebuildAndRender],
  );

  /**
   * 轻量装饰刷新（导航/滚动同步驱动）：只重灌窗格装饰（当前段侧标/查找命中），
   * 不重跑 diff 与行内计算。数据全部来自 derivedRef 缓存。
   */
  const refreshDecorations = useCallback(() => {
    const derived = derivedRef.current;
    const model = modelRef.current;
    if (!derived || !model) return;
    const findInfo = computeFind();
    if (findIdxRef.current >= findInfo.matches.length) {
      findIdxRef.current = 0;
      setFindIdx(0);
    }
    const current = findInfo.matches[findIdxRef.current] ?? null;
    const buildInput = (pane: 'left' | 'right'): PaneContentInput => ({
      docText: derived[pane].lines.join('\n'),
      rows: derived.rows,
      sectionImportance: derived.importance,
      marks: derived.marks[pane],
      folds: derived.folds,
      findMarks: findInfo.matches
        .filter((m) => m.pane === pane)
        .map((m) => ({ line: m.line, from: m.from, to: m.to, current: m === current })),
      currentSectionId:
        currentSectionIdRef.current !== null &&
        derived.diffSections.some((s) => s.id === currentSectionIdRef.current)
          ? currentSectionIdRef.current
          : null,
      langExtension: langExtRef.current,
      editable: editingRef.current,
      bookmarks: bookmarksRef.current,
    });
    paneL.current?.setContent(buildInput('left'));
    paneR.current?.setContent(buildInput('right'));
  }, [computeFind]);

  // ---- 客户端重 diff（降级路径：旧引擎无 text/edit 能力位，或行表分叉兜底）----
  const rediffModel = useCallback((model: TextSessionModel): void => {
    model.sections = toSections(diffLinesOps(model.leftLines, model.rightLines));
    model.summary = summarizeSections(model.sections);
  }, []);

  /** 每会话一份编辑同步状态（队列 + 单飞行）：切换会话后旧队列不得混入新会话 */
  const editSyncRef = useRef(new Map<string, EditSyncState>());
  const [rediffNotice, setRediffNotice] = useState<string | null>(null);

  /**
   * 编辑后重 diff（R3-18）：补丁交引擎重算，采纳其权威 sections 与重要性。
   * 队列合并、分叉收敛与重放上限见 editSync.ts；无能力位时回落客户端全量重算。
   */
  const rediffAndRender = useCallback(
    async (model: TextSessionModel, ops: TextEditOp[]): Promise<void> => {
      // 会话已切走时只追平缓存，不回灌当前窗格（避免覆盖新会话内容）
      const active = () => modelRef.current?.sessionId === model.sessionId;
      const clientFallback = async () => {
        rediffModel(model);
        putTextSession(model);
        if (active()) await rebuildAndRender(model);
      };
      if (!getEngineCapabilities()?.textEdit) {
        await clientFallback();
        return;
      }
      let st = editSyncRef.current.get(model.sessionId);
      if (!st) {
        st = newEditSyncState();
        editSyncRef.current.set(model.sessionId, st);
      }
      await flushEdits(st, ops, {
        edit: (edits) => editText({ sessionId: model.sessionId, edits }),
        lines: () => ({ left: model.leftLines, right: model.rightLines }),
        adopt: async (res) => {
          model.sections = res.sections;
          model.summary = res.summary;
          putTextSession(model);
          setRediffNotice(null);
          if (active()) await rebuildAndRender(model);
        },
        fallback: clientFallback,
        diverged: () => setRediffNotice(S.text.rediffResyncFailed),
      });
    },
    [rediffModel, rebuildAndRender],
  );

  const applySideContent = useCallback(
    (model: TextSessionModel, side: 'left' | 'right', lines: string[]): void => {
      if (side === 'left') model.leftLines = lines;
      else model.rightLines = lines;
      model.modifiedSides[side] = true;
      // R3-05：编辑必须登记到标签上，否则关闭/退出守卫看不到未落盘内容
      useShell.getState().markContentDirty(tab.id, model.modifiedSides);
    },
    [tab.id],
  );

  // ---- R3-04：保存 / 另存为（FR-TXT-020；落盘由引擎 text/save 执行）----
  const [saveNotice, setSaveNotice] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  /** 活动窗格侧（另存为的作用对象，与 BC 一致） */
  const activeSideRef = useRef<'left' | 'right'>('left');

  const doSave = useCallback(
    async (requests: SaveRequest[]): Promise<void> => {
      const model = modelRef.current;
      if (!model || requests.length === 0) {
        setSaveNotice(S.text.saveNothing);
        return;
      }
      setSaving(true);
      try {
        const out = await runTextSave(model.sessionId, requests, { save: saveText });
        applySavedEntries(model, out.saved);
        const st = useShell.getState();
        st.markContentDirty(tab.id, model.modifiedSides);
        // 另存为：标签上的路径/名称/标题同步改指，会话保存与关闭守卫才取到正确目标
        const refs = st.findTab(tab.id)?.tab.text;
        for (const e of out.saved) {
          const ref = e.side === 'left' ? refs?.left : refs?.right;
          if (ref && ref.path !== e.path) st.retargetTextSide(tab.id, e.side, e.path);
        }
        if (out.unsupported) {
          setSaveNotice(S.text.saveUnsupported);
          return;
        }
        if (out.error !== null) {
          setSaveNotice(`${S.text.saveFail}: ${out.error}`);
          return;
        }
        const paths = out.saved.map((e) => e.path).join('、');
        const backups = out.saved.filter((e) => e.backupPath !== undefined).length;
        setSaveNotice(
          backups > 0
            ? `${S.text.saved}：${paths}（${S.text.savedBackupSuffix} ${backups}）`
            : `${S.text.saved}：${paths}`,
        );
        // 子会话保存 → 父目录行回传（FR-SES-004）：父会话重扫后推 dir/delta，
        // 父标签即使不在前台也追平。父会话可能已被关闭，失败不得掩盖已成功的保存，
        // 也不阻塞保存反馈（大目录重扫耗时）。
        const parentParams = parentRescanParams(refs?.parentFolderSessionId);
        if (parentParams) void filterDir(parentParams).catch(() => undefined);
      } finally {
        setSaving(false);
      }
    },
    [tab.id],
  );

  const doSaveAs = useCallback(
    (side: 'left' | 'right') => {
      const model = modelRef.current;
      if (!model) return;
      useShell.getState().openPrompt({
        title: `${S.text.saveAsTitle}（${side === 'left' ? S.text.paneLeft : S.text.paneRight}）`,
        initialValue: side === 'left' ? model.leftPath : model.rightPath,
        onSubmit: (value) => {
          if (value === null) return;
          const path = value.trim();
          if (!path) return;
          void doSave([{ side, path }]);
        },
      });
    },
    [doSave],
  );

  // ---- 初始加载（缓存水合优先，避免引擎重开覆盖 UI 侧编辑）----
  useEffect(() => {
    let disposed = false;
    // 差异文件跳转在同一标签内换会话：旧模型不得继续以"就绪"呈现（FR-DIR-071）
    const sessionChanged = liveSessionRef.current !== text.sessionId;
    liveSessionRef.current = text.sessionId;
    setReady(false);
    setError(null);
    if (sessionChanged) {
      // 位置/书签/查找序号/提示都属于旧文件；显示偏好（换行/语法/上下文）跨文件保留。
      // 另存为只改路径不换会话：那条路径上不得清掉刚给出的保存反馈与书签。
      modelRef.current = null;
      setNavIndex(-1);
      syncCurrentSection(null);
      setBookmarks({});
      setFindIdx(0);
      setSaveNotice(null);
      setRediffNotice(null);
    }
    void (async () => {
      try {
        const cached = getTextSession(text.sessionId);
        if (cached) {
          modelRef.current = cached;
          collapsedRef.current = new Set(cached.collapsedKeys);
          contextRowsRef.current = cached.contextRows;
          setContextRows(cached.contextRows);
          await rebuildAndRender(cached);
          if (!disposed) setReady(true);
          return;
        }
        // openText 411（会话已存在：StrictMode 重挂载/重复打开）→ 直接复用引擎侧会话
        const rules = effectiveRulesFor(text.left.path, text.right.path);
        const loaded = await openText({
          sessionId: text.sessionId,
          left: text.left.path,
          right: text.right.path,
          rules,
        })
          .then((opened) => loadTextSession(opened, text.left.path, text.right.path))
          .catch((err) => {
            if (err instanceof RpcError && err.code === 411) {
              return loadTextSessionReuse(text.sessionId, text.left.path, text.right.path);
            }
            throw err;
          });
        if (disposed) return;
        // 登记参数必须与 open 完全一致（含 rules）：否则崩溃重放后重要性折叠口径漂移
        trackSession(text.sessionId, 'text', {
          left: text.left.path,
          right: text.right.path,
          rules,
        });
        const model = newTextSessionModel({
          sessionId: text.sessionId,
          leftPath: text.left.path,
          rightPath: text.right.path,
          leftName: text.left.name,
          rightName: text.right.name,
          leftLines: loaded.leftLines,
          rightLines: loaded.rightLines,
          sections: loaded.sections,
          summary: loaded.summary,
          detectedEncoding: loaded.encodings,
        });
        collapsedRef.current = defaultCollapsedKeys(
          buildAlignedDocs(model.sections, model.leftLines, model.rightLines).left.rows,
          contextRowsRef.current,
        );
        await rebuildAndRender(model);
        if (!disposed) setReady(true);
      } catch (err) {
        if (!disposed) setError(err instanceof Error ? err.message : String(err));
      }
    })();
    return () => {
      disposed = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [text.sessionId, text.left.path, text.right.path]);

  // ---- 用户编辑：patch → 撤销入栈 → 重 diff → 回灌 ----
  const onUserEdit = useCallback(
    (side: 'left' | 'right', rows: AlignedRow[], docBefore: string[], docAfter: string[]) => {
      const model = modelRef.current;
      if (!model || !editingRef.current) return;
      const sideLines = side === 'left' ? model.leftLines : model.rightLines;
      const res = patchSideFromDocEdit(rows, docBefore, docAfter, side, sideLines);
      if (!res) return;
      model.undo = tryCoalesce(
        model.undo,
        { side, patches: res.patches, lastAt: Date.now() },
        Date.now(),
      );
      applySideContent(model, side, res.newLines);
      void rediffAndRender(model, opsFromPatches(side, res.patches));
    },
    [applySideContent, rediffAndRender],
  );

  // ---- 会话级撤销 / 重做（UNDO-001）----
  const applyUndoEntry = useCallback(
    (entry: UndoEntry, patches: SidePatch[]): TextEditOp[] => {
      const model = modelRef.current;
      if (!model) return [];
      const cur = entry.side === 'left' ? model.leftLines : model.rightLines;
      applySideContent(model, entry.side, applySidePatches(cur, patches));
      return opsFromPatches(entry.side, patches);
    },
    [applySideContent],
  );

  const doUndo = useCallback(() => {
    const model = modelRef.current;
    if (!model) return;
    const popped = popUndo(model.undo);
    if (!popped) return;
    model.undo = popped.state;
    void rediffAndRender(model, applyUndoEntry(popped.entry, undoPatches(popped.entry)));
  }, [applyUndoEntry, rediffAndRender]);

  const doRedo = useCallback(() => {
    const model = modelRef.current;
    if (!model) return;
    const popped = popRedo(model.undo);
    if (!popped) return;
    model.undo = popped.state;
    void rediffAndRender(model, applyUndoEntry(popped.entry, redoPatches(popped.entry)));
  }, [applyUndoEntry, rediffAndRender]);

  // ---- 滚动同步（镜像 + 视口上报 + 当前差异段同步高亮）----
  const onPaneScroll = useCallback(
    (side: 'left' | 'right') => {
      if (syncingRef.current) return;
      syncingRef.current = true;
      const { left: hL, right: hR } = paneHandles();
      const src = side === 'left' ? hL : hR;
      const dst = side === 'left' ? hR : hL;
      const s = src?.getView()?.scrollDOM;
      const d = dst?.getView()?.scrollDOM;
      if (s && d) {
        d.scrollTop = s.scrollTop;
        d.scrollLeft = s.scrollLeft;
      }
      requestAnimationFrame(() => {
        syncingRef.current = false;
      });
      if (rafRef.current === null) {
        rafRef.current = requestAnimationFrame(() => {
          rafRef.current = null;
          const h = side === 'left' ? paneL.current : paneR.current;
          if (!h) return;
          const topRow = h.topVisibleLine();
          const bottomRow = Math.max(topRow, h.bottomVisibleLine());
          setViewport({ topRow, bottomRow });
          // 当前位置在差异列表的同步高亮（滚动驱动）
          const derived = derivedRef.current;
          if (derived && derived.rows.length > 0) {
            const rowIdx = Math.min(topRow, derived.rows.length - 1);
            const row = derived.rows[rowIdx];
            const sideLine = row.leftLine ?? row.rightLine ?? null;
            if (sideLine !== null) {
              const sec = findDiffSectionAtLine(derived.diffSections, 'left', sideLine);
              if (sec && sec.id !== currentSectionIdRef.current) {
                syncCurrentSection(sec.id);
                setNavIndex(derived.diffSections.findIndex((x) => x.id === sec.id));
                refreshDecorations();
              }
            }
          }
        });
      }
    },
    [refreshDecorations, syncCurrentSection],
  );

  // ---- 差异导航（跨折叠：折叠只覆盖等值区，差异行始终可见）----
  const revealRow = useCallback((row: number) => {
    const { left: hL, right: hR } = paneHandles();
    hL?.revealLine(row);
    hR?.revealLine(row);
  }, []);

  const gotoSection = useCallback(
    (idx: number) => {
      const derived = derivedRef.current;
      if (!derived || derived.diffSections.length === 0) return;
      const len = derived.diffSections.length;
      const wrapped = ((idx % len) + len) % len;
      const sec = derived.diffSections[wrapped];
      syncCurrentSection(sec.id);
      setNavIndex(wrapped);
      const firstRow = sec.left?.start ?? sec.right?.start ?? 0;
      // 目标行落入折叠区（重 diff 后边界移动）时先展开
      const folds = computeFolds(derived.rows, contextRowsRef.current, collapsedRef.current);
      const need = unfoldKeysFor(folds, [firstRow, Math.max(0, firstRow - 1)]);
      if (need.size > 0 && modelRef.current) {
        for (const k of need) collapsedRef.current.delete(k);
        void rebuildAndRender(modelRef.current);
      }
      revealRow(firstRow);
      refreshDecorations();
    },
    [rebuildAndRender, refreshDecorations, revealRow, syncCurrentSection],
  );

  // ---- 折叠操作（FR-TXT-020~023）----
  const refold = useCallback(() => {
    const model = modelRef.current;
    if (!model) return;
    void rebuildAndRender(model);
  }, [rebuildAndRender]);

  const onFoldAll = useCallback(() => {
    const derived = derivedRef.current;
    if (!derived) return;
    collapsedRef.current = allCollapsibleKeys(derived.rows, contextRowsRef.current);
    refold();
  }, [refold]);

  const onUnfoldAll = useCallback(() => {
    collapsedRef.current = new Set();
    refold();
  }, [refold]);

  const onUnfoldRange = useCallback(
    (fromRow: number) => {
      const derived = derivedRef.current;
      if (!derived) return;
      const folds = computeFolds(derived.rows, contextRowsRef.current, collapsedRef.current);
      const keys = unfoldKeysFor(folds, [fromRow]);
      for (const k of keys) collapsedRef.current.delete(k);
      refold();
    },
    [refold],
  );

  const onContextChange = useCallback(
    (n: number) => {
      const derived = derivedRef.current;
      contextRowsRef.current = n;
      setContextRows(n);
      if (derived) collapsedRef.current = allCollapsibleKeys(derived.rows, n);
      refold();
    },
    [refold],
  );

  // ---- 查找替换（FIND-001）----
  const findInfo = useMemo(
    () => (ready ? computeFind() : { matches: [], compiled: compileQuery(findState), error: null }),
    // uiVersion：重 diff 后匹配集合变化，需重算
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [ready, computeFind, findState, uiVersion],
  );

  const jumpToMatch = useCallback(
    (match: FindMatch) => {
      const derived = derivedRef.current;
      if (!derived) return;
      const folds = computeFolds(derived.rows, contextRowsRef.current, collapsedRef.current);
      const need = unfoldKeysFor(folds, [match.line]);
      if (need.size > 0 && modelRef.current) {
        for (const k of need) collapsedRef.current.delete(k);
        void rebuildAndRender(modelRef.current);
      }
      const h = match.pane === 'left' ? paneL.current : paneR.current;
      h?.selectRange(match.line, match.from, match.to);
      // 同步差异列表高亮（匹配所在差异段）
      const row = derived.rows[match.line];
      const sideLine = row ? (row.leftLine ?? row.rightLine ?? null) : null;
      if (sideLine !== null) {
        const sec = findDiffSectionAtLine(derived.diffSections, 'left', sideLine);
        if (sec) syncCurrentSection(sec.id);
      }
      refreshDecorations();
    },
    [rebuildAndRender, refreshDecorations, syncCurrentSection],
  );

  const onFind = useCallback(
    (backward: boolean) => {
      const derived = derivedRef.current;
      if (!derived || findInfo.matches.length === 0) return;
      const pane: 'left' | 'right' =
        findStateRef.current.side === 'both' ? 'left' : findStateRef.current.side;
      const h = pane === 'left' ? paneL.current : paneR.current;
      const cur = h ? h.cursorPos() : { line: 0, col: 0 };
      const next = findNext(derived[pane].lines, findInfo.compiled, cur.line, cur.col, backward);
      if (next) {
        const idx = findInfo.matches.findIndex(
          (m) => m.pane === pane && m.line === next.line && m.from === next.from,
        );
        const safe = Math.max(0, idx);
        findIdxRef.current = safe;
        setFindIdx(safe);
        jumpToMatch({ pane, ...next });
      }
      if (modelRef.current) void rebuildAndRender(modelRef.current);
    },
    [findInfo, jumpToMatch, rebuildAndRender],
  );

  const onReplaceCurrent = useCallback(() => {
    const model = modelRef.current;
    if (!model || !findInfo.compiled.ok) return;
    const match = findInfo.matches[findIdxRef.current];
    if (!match) return;
    const h = match.pane === 'left' ? paneL.current : paneR.current;
    const view = h?.getView();
    if (!view) return;
    const line = view.state.doc.line(match.line + 1);
    const matchedText = view.state.doc.sliceString(line.from + match.from, line.from + match.to);
    let groups: string[] = [];
    if (findStateRef.current.regex) {
      const re2 = new RegExp(
        findInfo.compiled.re.source,
        findInfo.compiled.re.flags.replace('g', ''),
      );
      const gm = re2.exec(matchedText);
      groups = gm ? gm.slice(1).map((g) => g ?? '') : [];
    }
    const rep = expandReplacement(
      matchedText,
      findStateRef.current.replacement,
      findStateRef.current.regex,
      groups,
    );
    // CM 事务 → onUserEdit 链路（patch → 撤销入栈 → 重 diff → 回灌）
    view.dispatch({
      changes: { from: line.from + match.from, to: line.from + match.to, insert: rep },
    });
  }, [findInfo]);

  const onReplaceAll = useCallback(() => {
    const model = modelRef.current;
    if (!model || !findInfo.compiled.ok) return;
    const q = findStateRef.current;
    const panes: Array<'left' | 'right'> = q.side === 'both' ? ['left', 'right'] : [q.side];
    let changed = false;
    const ops: TextEditOp[] = [];
    for (const pane of panes) {
      const cur = pane === 'left' ? model.leftLines : model.rightLines;
      const newLines = cur.map(
        (ln) => replaceAllInLine(ln, findInfo.compiled, q.replacement, q.regex) ?? ln,
      );
      const patches = patchesFromLineReplacement(cur, newLines);
      if (patches) {
        model.undo = pushUndo(model.undo, { side: pane, patches, lastAt: Date.now() });
        applySideContent(model, pane, newLines);
        ops.push(...opsFromPatches(pane, patches));
        changed = true;
      }
    }
    if (changed) void rediffAndRender(model, ops);
  }, [findInfo, applySideContent, rediffAndRender]);

  const onSelectAllMatchedLines = useCallback(() => {
    const derived = derivedRef.current;
    if (!derived || !findInfo.compiled.ok) return;
    const q = findStateRef.current;
    const panes: Array<'left' | 'right'> = q.side === 'both' ? ['left', 'right'] : [q.side];
    for (const pane of panes) {
      const lines = derived[pane].lines;
      const nos = matchedLineNos(lines, findInfo.compiled);
      if (nos.length === 0) continue;
      const h = pane === 'left' ? paneL.current : paneR.current;
      const view = h?.getView();
      if (!view) continue;
      const ranges = nos.map((n) => {
        const l = view.state.doc.line(Math.min(n + 1, view.state.doc.lines));
        return EditorSelection.range(l.from, l.to);
      });
      view.dispatch({
        selection: EditorSelection.create(ranges, 0),
        scrollIntoView: true,
      });
      h?.revealLine(nos[0]);
      h?.focusPane();
    }
  }, [findInfo]);

  // ---- 编码切换（ENC-001：会话级，带 encoding 重开重 diff；引擎按原编码回写）----
  const onEncodingChange = useCallback(
    async (enc: EncodingId) => {
      const model = modelRef.current;
      if (!model || enc === model.encoding) return;
      if (isTextModified(model) && !window.confirm(S.text.encodingDiscardConfirm)) return;
      const prevEnc = model.encoding;
      const rules = effectiveRulesFor(model.leftPath, model.rightPath);
      const reopen = (encoding: string | undefined) =>
        openText({
          sessionId: model.sessionId,
          left: model.leftPath,
          right: model.rightPath,
          rules,
          encoding,
        });
      try {
        await closeSession({ sessionId: model.sessionId });
        let reopened;
        try {
          reopened = await reopen(encodingParam(enc));
        } catch (err) {
          // 选错编码（413）时会话已关：按原编码恢复，否则视图停在无会话态
          await reopen(encodingParam(prevEnc)).catch(() => undefined);
          throw err;
        }
        const loaded = await loadTextSession(reopened, model.leftPath, model.rightPath);
        model.encoding = enc;
        model.detectedEncoding = loaded.encodings;
        model.leftLines = loaded.leftLines;
        model.rightLines = loaded.rightLines;
        model.sections = loaded.sections;
        model.summary = loaded.summary;
        model.undo = emptyUndoState();
        model.modifiedSides = { left: false, right: false };
        putTextSession(model);
        // 崩溃重放必须用切换后的编码，否则恢复出来的会话解码口径漂移
        trackSession(model.sessionId, 'text', {
          left: model.leftPath,
          right: model.rightPath,
          rules,
          encoding: encodingParam(enc),
        });
        useShell.getState().markContentDirty(tab.id, model.modifiedSides);
        collapsedRef.current = defaultCollapsedKeys(
          buildAlignedDocs(model.sections, model.leftLines, model.rightLines).left.rows,
          contextRowsRef.current,
        );
        syncCurrentSection(null);
        setNavIndex(-1);
        await rebuildAndRender(model);
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
      }
    },
    [rebuildAndRender, syncCurrentSection, tab.id],
  );

  // ---- DMP 清理开关（inlineDiff RPC 参数映射）----
  const onDmpToggle = useCallback(() => {
    const model = modelRef.current;
    if (!model) return;
    model.dmpCleanup = !model.dmpCleanup;
    void rebuildAndRender(model);
  }, [rebuildAndRender]);

  // ---- 查找输入实时刷新装饰（needle/开关变化 → 匹配高亮即时生效）----
  useEffect(() => {
    if (!ready || !modelRef.current) return;
    void rebuildAndRender(modelRef.current);
    // findState 为查找面板输入（needle/正则/大小写/整词/侧）——变化即重算匹配装饰
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [findState]);

  // ---- FR-TXT-002/023：自动换行与书签变化 → 刷新装饰（书签行标记即时生效）----
  useEffect(() => {
    refreshDecorations();
  }, [wrap, bookmarks, refreshDecorations]);

  // ---- 壳级关闭守卫落盘后同步本视图脏标记（R3-05：守卫保存不经过视图 doSave）----
  const storeContentDirty = useShell((st) => st.findTab(tab.id)?.tab.contentDirty);
  useEffect(() => {
    const m = modelRef.current;
    if (!m || !storeContentDirty) return;
    if (
      m.modifiedSides.left === storeContentDirty.left &&
      m.modifiedSides.right === storeContentDirty.right
    )
      return;
    m.modifiedSides = { ...storeContentDirty };
    setUiVersion((v) => v + 1);
  }, [storeContentDirty]);

  // ---- 壳级命令总线注册（换行/书签命令与快捷键统一入口）----
  useEffect(() => {
    const currentLine = () =>
      paneL.current?.cursorPos().line ?? paneR.current?.cursorPos().line ?? 0;
    const dispose = registerTextView({
      toggleWrap: () => setWrap((v) => !v),
      toggleBookmark: () => setBookmarks((m) => toggleBookmark(m, currentLine()).map),
      gotoBookmark: (dir) => {
        const line =
          dir === 1
            ? nextBookmarkLine(bookmarksRef.current, currentLine())
            : prevBookmarkLine(bookmarksRef.current, currentLine());
        if (line !== null) revealRow(line);
      },
      clearBookmarks: () => setBookmarks(clearBookmarks()),
      setBookmarkAtCursor: (slot) => setBookmarks((m) => setBookmarkSlot(m, slot, currentLine())),
      saveFile: () => {
        const m = modelRef.current;
        if (m) void doSave(saveRequestsFor(m.modifiedSides));
      },
      saveFileAs: () => doSaveAs(activeSideRef.current),
    });
    return dispose;
  }, [revealRow, doSave, doSaveAs]);

  // ---- Ctrl+F / F3 / Esc（视图级快捷键）----
  const onKeyDown = useCallback(
    (ev: React.KeyboardEvent) => {
      const mod = ev.ctrlKey || ev.metaKey;
      if (mod && ev.key.toLowerCase() === 'f') {
        ev.preventDefault();
        setFindOpen(true);
      } else if (ev.key === 'F3') {
        ev.preventDefault();
        if (findOpen) onFind(ev.shiftKey);
      } else if (ev.key === 'Escape' && findOpen) {
        setFindOpen(false);
      }
    },
    [findOpen, onFind],
  );

  // ---- 渲染 ----
  if (error) {
    return (
      <div className="aic-text">
        <div className="aic-error-box">
          <b>{S.text.loadFail}</b>
          <pre>{error}</pre>
        </div>
      </div>
    );
  }

  const model = modelRef.current;
  const derived = derivedRef.current;
  if (!ready || !model || !derived) {
    return (
      <div className="aic-text">
        <div className="aic-loading">{S.text.loading}</div>
      </div>
    );
  }

  const summary = model.summary;
  const diffCount = derived.diffSections.length;
  const hidden = hiddenRowCount(derived.folds);
  const encodingChoice = encodingById(model.encoding);
  const detectedLabels = [model.detectedEncoding.left, model.detectedEncoding.right]
    .filter((v): v is string => v !== null)
    .filter((v, i, a) => a.indexOf(v) === i)
    .map(encodingDisplayName);
  const encodingTitle =
    model.encoding === 'auto'
      ? detectedLabels.length > 0
        ? `${S.text.encodingDetected}${detectedLabels.join(' / ')} · ${S.text.encodingHintAuto}`
        : S.text.encodingHintAuto
      : `${encodingChoice.label} · ${S.text.encodingHintExplicit}`;
  const currentSec = derived.diffSections.find((s) => s.id === currentSectionId) ?? null;
  const currentRange = currentSec ? rowRangeOfSection(derived.rows, currentSec.id) : null;
  const inlineSource = getEngineCapabilities()?.inlineDiff
    ? S.text.inlineSourceRpc
    : S.text.inlineSourceClient;
  const rediffSource = getEngineCapabilities()?.textEdit
    ? S.text.rediffSourceRpc
    : S.text.rediffSourceClient;

  return (
    <div className={`aic-text${wrap ? ' wrap-on' : ''}`} onKeyDown={onKeyDown}>
      <div className="aic-text-toolbar">
        <span className="fname" title={text.left.path}>
          {text.left.name}
        </span>
        <span className="meta">⇄</span>
        <span className="fname" title={text.right.path}>
          {text.right.name}
        </span>
        <span className="meta">
          {model.leftLines.length.toLocaleString('zh-CN')} {S.text.linesUnit} ⇄{' '}
          {model.rightLines.length.toLocaleString('zh-CN')} {S.text.linesUnit}
        </span>
        {isTextModified(model) && (
          <span className="aic-dirty-chip" title={S.text.modifiedDot}>
            ●
          </span>
        )}
        <span className="meta">
          {S.text.sectionsLabel} <b>{diffCount}</b> · {S.text.equalSectionsLabel}{' '}
          <b>{summary.equalSections}</b>
        </span>
        {hidden > 0 && (
          <span className="meta dim" title={S.text.hiddenRowsLabel}>
            {S.text.hiddenRowsLabel} {hidden}
          </span>
        )}
        <span className="spacer" style={{ flex: 1 }} />
        <button className={`nav${editing ? ' on' : ''}`} onClick={() => setEditing((v) => !v)}>
          {editing ? S.text.editingOn : S.text.editingOff}
        </button>
        <button
          className="nav"
          onClick={doUndo}
          disabled={model.undo.undo.length === 0}
          title={S.text.undo}
        >
          ↶
        </button>
        <button
          className="nav"
          onClick={doRedo}
          disabled={model.undo.redo.length === 0}
          title={S.text.redo}
        >
          ↷
        </button>
        <button
          className="nav"
          onClick={() => void doSave(saveRequestsFor(model.modifiedSides))}
          disabled={saving || !isTextModified(model)}
          title={S.text.saveFileHint}
        >
          {S.text.saveFile}
        </button>
        <button
          className="nav"
          onClick={() => doSaveAs(activeSideRef.current)}
          disabled={saving}
          title={S.text.saveFileAsHint}
        >
          {S.text.saveFileAs}
        </button>
        <button className="nav" onClick={() => setFindOpen((v) => !v)} title={S.text.find}>
          {S.text.find}
        </button>
        <button
          className="nav"
          onClick={() => setShowDiffList((v) => !v)}
          title={S.text.diffListTitle}
        >
          {S.text.diffListTitle}
        </button>
        {/* FR-TXT-002 自动换行 / FR-TXT-023 书签 */}
        <button
          className={`nav${wrap ? ' on' : ''}`}
          onClick={() => setWrap((v) => !v)}
          title={S.text.wrapHint}
        >
          {S.text.wrap}
        </button>
        <button
          className="nav"
          onClick={() =>
            setBookmarks(
              (m) =>
                toggleBookmark(
                  m,
                  paneL.current?.cursorPos().line ?? paneR.current?.cursorPos().line ?? 0,
                ).map,
            )
          }
          title={S.text.bookmarkHint}
        >
          {S.text.toggleBookmark}
        </button>
        <button
          className="nav"
          onClick={() => {
            const line = prevBookmarkLine(bookmarks, paneL.current?.cursorPos().line ?? 0);
            if (line !== null) revealRow(line);
          }}
          title={S.text.prevBookmarkHint}
        >
          {S.text.prevBookmark}
        </button>
        <button
          className="nav"
          onClick={() => {
            const line = nextBookmarkLine(bookmarks, paneL.current?.cursorPos().line ?? 0);
            if (line !== null) revealRow(line);
          }}
          title={S.text.nextBookmarkHint}
        >
          {S.text.nextBookmark}
        </button>
        <button
          className="nav"
          onClick={() => setBookmarks(clearBookmarks())}
          title={S.text.clearBookmarksHint}
        >
          {S.text.clearBookmarks}
        </button>
        <select
          className="aic-mini-select"
          value=""
          title={S.text.setBookmarkHint}
          onChange={(e) => {
            const slot = Number(e.target.value);
            if (!Number.isInteger(slot)) return;
            const line = paneL.current?.cursorPos().line ?? paneR.current?.cursorPos().line ?? 0;
            setBookmarks((m) => setBookmarkSlot(m, slot, line));
          }}
        >
          <option value="">{S.text.setBookmark}</option>
          {BOOKMARK_SLOTS.map((slot) => (
            <option key={slot} value={slot}>
              {slot}
            </option>
          ))}
        </select>
        <span className="meta dim" title={S.text.bookmarkLabelHint}>
          {bookmarkLabel(bookmarks) || S.text.noBookmarks}
        </span>
      </div>

      <div className="aic-text-toolbar aic-text-toolbar2">
        <button className="nav" onClick={() => gotoSection(0)} disabled={diffCount === 0}>
          {S.text.firstDiff}
        </button>
        <button
          className="nav"
          onClick={() => gotoSection((navIndex === -1 ? 0 : navIndex) - 1)}
          disabled={diffCount === 0}
        >
          {S.text.prevDiff}
        </button>
        <button
          className="nav"
          onClick={() => gotoSection(navIndex === -1 ? 1 : navIndex + 1)}
          disabled={diffCount === 0}
        >
          {S.text.nextDiff}
          {navIndex >= 0 ? `（${navIndex + 1}/${diffCount}）` : ''}
        </button>
        {text.parentFolderSessionId && (
          <>
            <span className="meta sep">|</span>
            <button
              className="nav"
              title={S.text.prevDiffFile}
              onClick={() => void useShell.getState().navigateDiffFile(tab.id, 'prev')}
            >
              {S.text.navPrevShort}
            </button>
            <button
              className="nav"
              title={S.text.nextDiffFile}
              onClick={() => void useShell.getState().navigateDiffFile(tab.id, 'next')}
            >
              {S.text.navNextShort}
            </button>
            <button
              className="nav"
              title={S.text.copyRightNext}
              onClick={() => void useShell.getState().navigateDiffFile(tab.id, 'next', 'right')}
            >
              {S.text.navCopyRightShort}
            </button>
            <button
              className="nav"
              title={S.text.copyLeftNext}
              onClick={() => void useShell.getState().navigateDiffFile(tab.id, 'next', 'left')}
            >
              {S.text.navCopyLeftShort}
            </button>
          </>
        )}
        <span className="meta sep">|</span>
        <button className="nav" onClick={onFoldAll}>
          {S.text.foldAll}
        </button>
        <button className="nav" onClick={onUnfoldAll}>
          {S.text.unfoldAll}
        </button>
        <span className="meta">
          {S.text.contextLabel}
          <select
            className="aic-mini-select"
            value={contextRows}
            onChange={(e) => onContextChange(Number(e.target.value))}
          >
            {[0, 1, 2, 3, 5, 8].map((n) => (
              <option key={n} value={n}>
                {n}
              </option>
            ))}
          </select>
        </span>
        <span className="meta sep">|</span>
        <button className={`nav${syntaxOn ? ' on' : ''}`} onClick={() => setSyntaxOn((v) => !v)}>
          {S.text.syntaxLabel}
        </button>
        <span className="meta" title={encodingTitle}>
          {S.text.encodingLabel}
          <select
            className="aic-mini-select"
            value={model.encoding}
            onChange={(e) => void onEncodingChange(e.target.value as EncodingId)}
          >
            {ENCODING_CHOICES.map((e) => (
              <option key={e.id} value={e.id}>
                {e.label}
              </option>
            ))}
          </select>
          {model.encoding === 'auto' && detectedLabels.length > 0 && (
            <span className="dim">{detectedLabels.join(' / ')}</span>
          )}
        </span>
        <button
          className={`nav${model.dmpCleanup ? ' on' : ''}`}
          onClick={onDmpToggle}
          title={S.text.dmpCleanup}
        >
          {S.text.dmpCleanup}
        </button>
        <span className="meta dim">{inlineSource}</span>
        <span className="meta dim">{rediffSource}</span>
        {rediffNotice && <span className="meta warn">{rediffNotice}</span>}
        {saveNotice && (
          <span
            className={`meta${saveNotice.startsWith(S.text.saveFail) || saveNotice === S.text.saveUnsupported ? ' warn' : ' dim'}`}
          >
            {saveNotice}
          </span>
        )}
      </div>

      {findOpen && (
        <FindPanel
          state={findState}
          onChange={setFindState}
          result={{ total: findInfo.matches.length, index: findIdx, error: findInfo.error }}
          onFind={onFind}
          onReplaceCurrent={onReplaceCurrent}
          onReplaceAll={onReplaceAll}
          onSelectAllMatchedLines={onSelectAllMatchedLines}
          onClose={() => setFindOpen(false)}
        />
      )}

      <div className={`aic-text-panes${showDiffList ? ' with-list' : ''}`}>
        <AlignedPane
          side="left"
          head={`${S.text.paneLeft} · ${text.left.name}`}
          initial={EMPTY_INPUT}
          hooks={{
            onUndo: doUndo,
            onRedo: doRedo,
            onUserEdit,
            onScroll: onPaneScroll,
            onUnfold: onUnfoldRange,
            onSelection: () => {
              activeSideRef.current = 'left';
            },
          }}
          registerHandle={registerPaneHandle('left')}
        />
        <AlignedPane
          side="right"
          head={`${S.text.paneRight} · ${text.right.name}`}
          initial={EMPTY_INPUT}
          hooks={{
            onUndo: doUndo,
            onRedo: doRedo,
            onUserEdit,
            onScroll: onPaneScroll,
            onUnfold: onUnfoldRange,
            onSelection: () => {
              activeSideRef.current = 'right';
            },
          }}
          registerHandle={registerPaneHandle('right')}
        />
        <OverviewBar
          rows={derived.rows}
          importance={derived.importance}
          folds={derived.folds}
          currentRange={currentRange}
          viewport={viewport}
          onJump={(row) => {
            const foldsNow = computeFolds(
              derived.rows,
              contextRowsRef.current,
              collapsedRef.current,
            );
            const need = unfoldKeysFor(foldsNow, [row]);
            for (const k of need) collapsedRef.current.delete(k);
            if (need.size > 0 && modelRef.current) void rebuildAndRender(modelRef.current);
            revealRow(row);
          }}
        />
        {showDiffList && (
          <DiffListPanel
            sections={derived.diffSections}
            currentId={currentSectionId}
            onGoto={gotoSection}
          />
        )}
      </div>
    </div>
  );
}

/** 空初始内容（数据经初始加载后的 setContent 回灌） */
const EMPTY_INPUT: PaneContentInput = {
  docText: '',
  rows: [],
  sectionImportance: new Map(),
  marks: [],
  folds: [],
  findMarks: [],
  currentSectionId: null,
  langExtension: null,
  editable: true,
};

/** 当前 section 的对齐行区间（概览条高亮框） */
function rowRangeOfSection(
  rows: AlignedRow[],
  sectionId: number,
): { fromRow: number; toRow: number } | null {
  let from = -1;
  let to = -1;
  for (let i = 0; i < rows.length; i++) {
    if (rows[i].sectionId === sectionId) {
      if (from < 0) from = i;
      to = i;
    }
  }
  return from >= 0 ? { fromRow: from, toRow: to } : null;
}
