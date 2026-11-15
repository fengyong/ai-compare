import { useEffect, useLayoutEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react';
import { useVirtualizer } from '@tanstack/react-virtual';
import {
  type DirExecuteOp,
  type DirExecuteResult,
  type DirRow,
  type SyncPreset,
} from '../../protocol/types';
import type { Tab } from '../../shell/store';
import { useShell } from '../../shell/store';
import { ContextMenu, type ContextMenuItem } from '../../shell/ContextMenu';
import {
  compareDirContent,
  executeOperations,
  expandDir,
  filterDir,
  getEngineCapabilities,
  openFolder,
  planOperations,
  trackSession,
} from '../../ipc/engineProcess';
import { formatSize, formatTime } from '../../lib/env';
import { S } from '../../strings';
import { ensureDirCacheAttached } from './dirRowCache';
import { DirStatsBar } from './DirStatsBar';
import { statusDotClass, statusVisual, toneClass } from './statusDisplay';
import {
  buildOpsForCommand,
  hasIrreversible,
  opsFromPlan,
  otherSide,
  summarizeOps,
  SYNC_PRESETS,
  SYNC_PRESET_LABELS,
  type OpCommandKind,
  type Side,
} from './dirOps';
import { displayFilterPreset, parseMaskInput, SHOW_ALL_DISPLAY_FILTER } from './dirFilters';
import {
  buildTreeIndex,
  expandAll,
  orderedFlatRows,
  orderedTreeRows,
  toggleExpanded,
  collapseAll,
  rowDepth,
  type DirSortKey,
  type DirSortSpec,
} from './dirTree';
import {
  gutterCopyDirs,
  nextSortSpec,
  paneColumns,
  scrollSyncPlan,
  sidePresent,
  withPaneSelection,
  type PaneSelections,
  type PaneSide,
} from './dirPanes';
import {
  clearSelection,
  emptySelection,
  invertSelection,
  onRowClick,
  runEditSelectCommand,
  selectAll,
  type EditSelectCommand,
  type SelectionState,
} from './dirSelection';
import {
  ConfirmOpsDialog,
  PlanPreviewDialog,
  RenameDialog,
  type ConfirmOpsDialogState,
  type PlanPreviewDialogState,
  type RenameDialogState,
} from './DirOpsDialogs';
import { BatchRenameDialog, type BatchRenameDialogState } from './BatchRenameDialog';
import { QuickCompareDialog, type QuickCompareDialogState } from './QuickCompareDialog';
import { ContentCompareDialog, type ContentCompareDialogState } from './ContentCompareDialog';
import { effectiveDirCriteria, effectiveDirRules } from './dirCriteria';
import {
  buildCompareContentParams,
  compareTargetIds,
  contentCompareSupported,
} from './contentCompare';
import { BasePathBar } from './BasePathBar';
import { renamesToOps } from './renameTemplate';
import { copyText } from './clipboard';
import { F } from './localStrings';
import { registerFolderView, type FolderViewActions } from './folderViewBus';

/**
 * 文件夹比较视图（docs/03 §3.4.2 folder-compare / P6 完备化 + P6b 遗留批）：
 * - BC 双窗格（FR-DIR-001/003）：左右各自成列（名称/大小/已修改）、各自选择集、
 *   各自横向滚动；中间 gutter 显示内容状态图标 + 左右复制箭头。
 *   行按名称对齐（FR-DIR-020，一份 DirRow 两侧渲染），孤立项在缺侧整行留白；
 *   纵向滚动三列同步（dirPanes.scrollSyncPlan，跳过源与已对齐者防事件回环）；
 * - 虚拟滚动表（TanStack Virtual）：10 万行目标级窗口渲染（PB-04），
 *   虚拟窗口挂在左窗格，右窗格与 gutter 复用同一批 virtual items；
 * - 树形/平铺切换（FR-DIR-021）：树形按展开状态 DFS 序（dirTree.ts），
 *   惰性会话（lazyScan:true）双击目录消费 dir/expand；展开状态两侧共用；
 * - 选择模型（FR-DIR-050）：单击/Ctrl 累加/Shift 连选（dirSelection.ts），
 *   工具栏选择命令/空格标记/右键操作作用于「活动窗格」（最后一次点击的那侧）；
 * - P6b 编辑菜单（§3.1.6）：选择较新的/较旧的/孤立的（左右/两侧）/
 *   所有文件/全选/反选——runEditSelectCommand 数据面 + 计数反馈；
 * - P6b 空格标记：Space 切换选中行标记集（会话缓存层持久，
 *   橙色角标；仅显示标记过滤；与表头排序正交——均以行 id 为键）；
 * - P6b 批量重命名（FR-DIR-064）：DOS 掩码/正则模板 + 预览冲突拦截
 *   → rename ops 走既有确认 → dir/execute 批次链；
 * - P6b 快速比较（FR-DIR-013）：摘要对话框（size/mtime/状态 + 快速判定说明）；
 * - P6b 复制文件名/路径（FR-DIR-066）：右键菜单 + navigator.clipboard（降级提示）；
 * - P6b 基文件夹导航（FR-DIR-003）：可编辑面包屑，点击层级/上一级/交换侧
 *   → openFolderTab 打开新会话（两侧基路径可各自独立切换）；
 * - 右键操作（FR-DIR-060~066）：复制/移动/删除/重命名/对时 → 确认对话框
 *   （含不可逆警示）→ dir/execute（mock 同形；真实引擎待协议合入）；
 * - 同步预设（FR-DIR-067）：5 预设按钮 → dir/operate 计划预览 → 执行；
 * - 过滤器（FR-DIR-031/033/035）：名称过滤输入栏 + 显示过滤预设 + 抑制
 *   （青绿还原）→ dir/filter（引擎侧求值；mock 同形实现）；
 * - P6b 表头排序（FR-DIR-042）：点击任一侧列标题循环 升序→降序→默认，
 *   两侧共用一份排序规格以保持行对齐，标记不受影响。
 */

interface ExecReport {
  ok: number;
  failed: number;
  skipped: number;
  details: { status: string; kind: string; path: string; detail: string | null }[];
}

/** P6b 标记角标橙色（BC 标记惯例；CSS 归 theme/global.css（他线独占），内联样式） */
const MARK_ORANGE = '#e8842c';

const markBadgeStyle: React.CSSProperties = {
  fontSize: 10,
  color: MARK_ORANGE,
  border: `1px solid ${MARK_ORANGE}`,
  borderRadius: 8,
  padding: '0 5px',
  marginLeft: 6,
  flex: 'none',
};

const editMenuItemStyle: React.CSSProperties = {
  border: 'none',
  background: 'transparent',
  color: 'inherit',
  textAlign: 'left',
  padding: '4px 10px',
  borderRadius: 4,
  cursor: 'pointer',
  font: 'inherit',
  fontSize: 12,
  whiteSpace: 'nowrap',
};

/** 编辑菜单命令表（§3.1.6：较新的/较旧的/孤立的，左/右/两侧；全选等复用既有按钮文案） */
const EDIT_MENU_ITEMS: { key: string; label: string; cmd?: EditSelectCommand; sep?: boolean }[] = [
  { key: 'files', label: F.editSelectFiles, cmd: 'selectFiles' },
  { key: 'sep1', label: '', sep: true },
  { key: 'newerL', label: F.editSelectNewer('左'), cmd: 'selectNewerLeft' },
  { key: 'newerR', label: F.editSelectNewer('右'), cmd: 'selectNewerRight' },
  { key: 'newerB', label: F.editSelectNewer('两侧'), cmd: 'selectNewerBoth' },
  { key: 'sep2', label: '', sep: true },
  { key: 'olderL', label: F.editSelectOlder('左'), cmd: 'selectOlderLeft' },
  { key: 'olderR', label: F.editSelectOlder('右'), cmd: 'selectOlderRight' },
  { key: 'olderB', label: F.editSelectOlder('两侧'), cmd: 'selectOlderBoth' },
  { key: 'sep3', label: '', sep: true },
  { key: 'orphanL', label: F.editSelectOrphan('左'), cmd: 'selectOrphanLeft' },
  { key: 'orphanR', label: F.editSelectOrphan('右'), cmd: 'selectOrphanRight' },
  { key: 'orphanB', label: F.editSelectOrphan('两侧'), cmd: 'selectOrphanBoth' },
];

/** 单侧窗格最小内容宽度（名称 200 + 大小 92 + 已修改 132 + 间距 16 + 内边距 24）：
 *  窗格窄于此值时出现该侧独立的横向滚动条（FR-DIR-003 两侧互不牵连） */
const PANE_MIN_WIDTH = 464;

/** 活动窗格标识（工具栏选择命令/空格标记/右键操作的作用侧） */
const SIDE_LABEL: Record<PaneSide, string> = { left: S.folder.leftTag, right: S.folder.rightTag };

export function FolderCompareView({ tab }: { tab: Tab }) {
  const folder = tab.folder!;
  const openChild = useShell((st) => st.openChild);
  const setNotice = useShell((st) => st.setNotice);
  // P6b：基文件夹导航（FR-DIR-003）——复用既有 store 动作（新标签 = 新会话）
  const openFolderTab = useShell((st) => st.openFolderTab);

  // sessionId 由客户端分配（生成协议 DirOpenParams），标签创建时即确定；
  // delta 前送在 dir/open 之前挂好，无应答/首批通知的竞态
  const cache = ensureDirCacheAttached(folder.sessionId!);
  // R3-28：视图偏好从会话缓存 rehydrate（切换标签卸载视图不丢；首次为 null → 默认）
  const savedView = cache.getViewState();

  const [error, setError] = useState<string | null>(null);
  const [ready, setReady] = useState(false);
  // 两侧独立选择集 + 活动窗格（FR-DIR-003/050）
  const [paneSel, setPaneSel] = useState<PaneSelections>(
    savedView?.paneSel ?? { left: emptySelection, right: emptySelection },
  );
  const [activeSide, setActiveSide] = useState<PaneSide>(savedView?.activeSide ?? 'left');
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(savedView?.expanded ?? new Set());
  const [flat, setFlat] = useState(savedView?.flat ?? false);
  const [suppress, setSuppress] = useState(savedView?.suppress ?? false);
  const [displayPreset, setDisplayPreset] = useState(savedView?.displayPreset ?? 'all');
  const [filterText, setFilterText] = useState(savedView?.filterText ?? '');
  const [appliedFilterText, setAppliedFilterText] = useState(savedView?.appliedFilterText ?? '');
  const [execReport, setExecReport] = useState<ExecReport | null>(null);
  const [showDetail, setShowDetail] = useState(false);
  const [running, setRunning] = useState<number | null>(null);
  const [confirmState, setConfirmState] = useState<ConfirmOpsDialogState | null>(null);
  const [planState, setPlanState] = useState<PlanPreviewDialogState | null>(null);
  const [renameState, setRenameState] = useState<RenameDialogState | null>(null);
  const [ctxMenu, setCtxMenu] = useState<{ x: number; y: number; rowId: string } | null>(null);
  // ---- P6b：编辑菜单 / 标记过滤 / 表头排序 / 批量重命名 / 快速比较 ----
  const [editMenuOpen, setEditMenuOpen] = useState(false);
  const [markOnly, setMarkOnly] = useState(savedView?.markOnly ?? false);
  const [sortSpec, setSortSpec] = useState<DirSortSpec | null>(savedView?.sortSpec ?? null);
  const [batchRenameState, setBatchRenameState] = useState<BatchRenameDialogState | null>(null);
  const [quickCompareState, setQuickCompareState] = useState<QuickCompareDialogState | null>(null);
  const [contentCompareState, setContentCompareState] = useState<ContentCompareDialogState | null>(
    null,
  );

  // 活动窗格的选择集 = 工具栏/键盘/右键操作的默认目标
  const selection = paneSel[activeSide];
  const setSelection = (next: SelectionState, side: PaneSide = activeSide): void => {
    setPaneSel((all) => withPaneSelection(all, side, next));
  };

  const version = useSyncExternalStore(cache.subscribe, cache.getSnapshot);
  // 标记集（会话缓存层持久；toggle/setMarks 推版本号触发重渲染）
  const marks = cache.getMarks();
  const markCount = cache.getMarkCount();

  // R3-28：视图偏好回写会话缓存（切换标签卸载视图前持久；不推版本号，无谓重渲染）。
  // 仅视图偏好，不含瞬态对话框/执行反馈；会话关闭随 releaseDirCache 一并释放。
  useLayoutEffect(() => {
    cache.setViewState({
      expanded,
      flat,
      suppress,
      displayPreset,
      filterText,
      appliedFilterText,
      sortSpec,
      markOnly,
      paneSel,
      activeSide,
    });
  }, [
    cache,
    expanded,
    flat,
    suppress,
    displayPreset,
    filterText,
    appliedFilterText,
    sortSpec,
    markOnly,
    paneSel,
    activeSide,
  ]);

  // ---- 会话建立（ensureOpened 幂等：StrictMode 双调用安全；P6 惰性会话） ----
  useEffect(() => {
    let disposed = false;
    // R3-16：判据来自生效设置（含内容比较方法），不再硬编码默认值
    const criteria = effectiveDirCriteria(tab.sessionLink?.sessionId);
    const rules = effectiveDirRules(tab.sessionLink?.sessionId);
    void cache
      .ensureOpened(() =>
        openFolder({
          sessionId: cache.sessionId,
          left: folder.left,
          right: folder.right,
          depth: 1,
          criteria,
          rules,
          // P6：惰性会话 —— 顶层立即应答，目录展开经 dir/expand 惰性补推
          lazyScan: true,
        }),
      )
      .then(() => {
        if (!disposed) {
          trackSession(cache.sessionId, 'dir', {
            left: folder.left,
            right: folder.right,
            depth: 1,
            criteria,
            rules,
            lazyScan: true,
          });
          setReady(true);
          setError(null);
        }
      })
      .catch((err: unknown) => {
        if (!disposed) setError(err instanceof Error ? err.message : String(err));
      });
    return () => {
      disposed = true;
    };
  }, [cache, folder.left, folder.right, tab.sessionLink?.sessionId]);

  // ---- 可见行（树形 DFS 序 / 平铺名称序；表头排序 + 仅显示标记过滤；版本号触发重算） ----
  const allRows = cache.getRows();
  const visibleRows = useMemo(() => {
    void version; // 依赖版本号触发重算（rows 数组原地更新）
    // 表头排序（FR-DIR-042）：树形=逐层排序（目录先、父先于子不变）；平铺=全列表排序。
    // 标记/选择均以行 id 为键，与排序正交。
    const base = flat
      ? orderedFlatRows(allRows, sortSpec)
      : orderedTreeRows(buildTreeIndex(allRows, sortSpec), expanded);
    // 仅显示标记（P6b）：客户端过滤，不影响引擎侧显示过滤器
    return markOnly ? base.filter((r) => marks.has(r.id)) : base;
  }, [allRows, flat, expanded, version, sortSpec, markOnly, marks]);
  const visibleIds = useMemo(() => visibleRows.map((r) => r.id), [visibleRows]);
  const visibleIdSet = useMemo(() => new Set(visibleIds), [visibleIds]);

  /** 编辑菜单数据面：全量行 + 当前可见性（显示过滤/标记过滤后的实际可见） */
  const editMenuRows = useMemo(
    () =>
      allRows.map((r) =>
        visibleIdSet.has(r.id) ? { ...r, visible: true } : { ...r, visible: false },
      ),
    // version 驱动 visibleIdSet 变化（allRows 原地更新时引用不变），显式声明意图
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [allRows, visibleIdSet, version],
  );

  // ---- 虚拟滚动（窗口挂左窗格；右窗格与 gutter 复用同一批 items） ----
  const leftScrollRef = useRef<HTMLDivElement>(null);
  const rightScrollRef = useRef<HTMLDivElement>(null);
  const gutterScrollRef = useRef<HTMLDivElement>(null);
  const leftHeadRef = useRef<HTMLDivElement>(null);
  const rightHeadRef = useRef<HTMLDivElement>(null);
  const virtualizer = useVirtualizer({
    count: ready ? visibleRows.length : 0,
    getScrollElement: () => leftScrollRef.current,
    estimateSize: () => 28,
    overscan: 12,
  });
  const items = virtualizer.getVirtualItems();

  useLayoutEffect(() => {
    virtualizer.measure();
  }, [virtualizer, visibleRows.length]);

  // ---- 滚动同步：纵向三列对齐（行对齐的前提），横向各侧表头跟随本格 ----
  const syncVertical = (sourceId: string, el: HTMLElement): void => {
    const nodes: Record<string, HTMLDivElement | null> = {
      left: leftScrollRef.current,
      gutter: gutterScrollRef.current,
      right: rightScrollRef.current,
    };
    const present = Object.entries(nodes).filter(
      (entry): entry is [string, HTMLDivElement] => entry[1] !== null,
    );
    const plan = scrollSyncPlan(
      sourceId,
      el.scrollTop,
      present.map(([id, node]) => ({ id, top: node.scrollTop })),
    );
    for (const p of plan) nodes[p.id]!.scrollTop = p.top;
  };

  const syncHorizontal = (side: PaneSide, el: HTMLElement): void => {
    const head = side === 'left' ? leftHeadRef.current : rightHeadRef.current;
    if (!head) return;
    for (const p of scrollSyncPlan('body', el.scrollLeft, [{ id: 'head', top: head.scrollLeft }])) {
      head.scrollLeft = p.top;
    }
  };

  const onPaneScroll = (side: PaneSide, el: HTMLElement): void => {
    syncVertical(side, el);
    syncHorizontal(side, el);
  };

  // ---- 展开/折叠（dir/expand；mock/真实引擎 upsert 幂等） ----
  const toggleDir = (row: DirRow): void => {
    if (!row.hasChildren) return;
    // 展开地址取真实 rel（树形 id 恰好等于 rel，但地址模型以侧路径为准，R3-03）
    const path = row.leftPath ?? row.rightPath;
    if (!path) return;
    const isOpen = expanded.has(row.id);
    setExpanded(toggleExpanded(expanded, row.id));
    if (!isOpen) {
      // 惰性会话消费 dir/expand（真实引擎推子项 delta；mock 重推幂等）
      expandDir({ sessionId: cache.sessionId, path, depth: 1 }).catch((err: unknown) => {
        setNotice(S.folder.expandFailed(err instanceof Error ? err.message : String(err)));
      });
    }
  };

  // ---- 选择（按窗格独立；点击即把该侧设为活动窗格） ----
  const rowClick = (side: PaneSide, row: DirRow, e: React.MouseEvent): void => {
    setActiveSide(side);
    const next = onRowClick(
      paneSel[side],
      row.id,
      { ctrl: e.ctrlKey || e.metaKey, shift: e.shiftKey },
      visibleIds,
    );
    setSelection(next, side);
  };

  const selectedCount = selection.selected.size;

  const targetRows = (): { rows: readonly DirRow[]; ids: Set<string> } => {
    // 右键单行（不在选择中）= 以该行为目标的临时单选
    if (selectedCount === 0 && ctxMenu) {
      return { rows: allRows, ids: new Set([ctxMenu.rowId]) };
    }
    return { rows: allRows, ids: new Set(selection.selected) };
  };

  // ---- 过滤器（dir/filter：名称 + 显示 + 抑制 + 平铺） ----
  const applyFilters = async (patch?: {
    flat?: boolean;
    suppress?: boolean;
    display?: string;
    filterText?: string;
    rescan?: boolean;
  }): Promise<void> => {
    const nextFlat = patch?.flat ?? flat;
    const nextSuppress = patch?.suppress ?? suppress;
    const nextDisplay = patch?.display ?? displayPreset;
    const nextFilterText = patch?.filterText ?? appliedFilterText;
    const spec = displayFilterPreset(nextDisplay);
    const df = nextDisplay === 'all' ? SHOW_ALL_DISPLAY_FILTER : spec;
    try {
      const result = await filterDir({
        sessionId: cache.sessionId,
        nameFilter: parseMaskInput(nextFilterText),
        displayFilter: df,
        suppress: nextSuppress,
        flat: nextFlat,
        rescan: patch?.rescan ?? undefined,
      });
      setNotice(S.folder.filterResult(result.visibleRows, result.hiddenRows));
    } catch (err: unknown) {
      setNotice(S.folder.opsExecuteFailed(err instanceof Error ? err.message : String(err)));
    }
  };

  // ---- 操作执行（确认 -> dir/execute -> 反馈条） ----
  const runOps = (ops: DirExecuteOp[], title: string): void => {
    const summary = summarizeOps(ops);
    setConfirmState({
      title,
      ops,
      summary,
      irreversible: hasIrreversible(ops),
      onConfirm: () => {
        setConfirmState(null);
        void doExecute(ops);
      },
      onCancel: () => setConfirmState(null),
    });
  };

  const doExecute = async (ops: DirExecuteOp[]): Promise<void> => {
    setRunning(ops.length);
    setShowDetail(false);
    try {
      const result: DirExecuteResult = await executeOperations({ sessionId: cache.sessionId, ops });
      setExecReport({
        ok: result.succeeded.length,
        failed: result.failed.length,
        skipped: result.skipped.length,
        details: [...result.failed, ...result.skipped, ...result.succeeded].map((r) => ({
          status: r.status,
          kind: r.op.kind,
          path: r.op.path,
          detail: r.detail,
        })),
      });
      if (result.failed.length > 0) setShowDetail(true);
    } catch (err: unknown) {
      const isMethodNotFound =
        err instanceof Error && 'code' in err && (err as { code?: number }).code === -32601;
      setNotice(
        isMethodNotFound
          ? S.folder.opsEngineUnsupported
          : S.folder.opsExecuteFailed(err instanceof Error ? err.message : String(err)),
      );
    } finally {
      setRunning(null);
    }
  };

  const commandOps = (
    cmd: OpCommandKind,
    arg?: { newName?: string; side?: Side },
  ): DirExecuteOp[] => {
    const { rows, ids } = targetRows();
    return buildOpsForCommand(cmd, rows, ids, arg);
  };

  // ---- P6b：编辑菜单命令（§3.1.6）→ 选择状态更新 + 计数反馈 ----
  const applyEditCommand = (cmd: EditSelectCommand, label: string): void => {
    const next = runEditSelectCommand(cmd, selection, editMenuRows, visibleIds);
    setSelection(next);
    setEditMenuOpen(false);
    // 文案局部常量（待并入 strings.ts）
    setNotice(`${label} — ${F.editSelectedByCommand(next.selected.size)}`);
  };

  // ---- P6b：空格标记（BC 键盘惯例；Space 切换选中行标记集） ----
  const toggleSelectionMarks = (): void => {
    const ids = [...selection.selected];
    if (ids.length === 0) return;
    const target = !cache.isMarked(ids[0]);
    cache.setMarks(ids, target);
    // 文案局部常量（待并入 strings.ts）
    setNotice(target ? F.markedCount(ids.length) : `已取消 ${ids.length} 项标记`);
  };
  // ---- 壳级命令总线（Ctrl+L/Ctrl+R 复制到对侧、Space 标记；附录 G / FR-DIR-050）----
  // 动作闭包每次渲染刷新到 ref，注册只做一次：多标签下重复注册会把本视图顶到
  // 栈顶，抢走仍在挂载的其它目录视图的命令。
  const folderActionsRef = useRef<FolderViewActions | null>(null);
  folderActionsRef.current = {
    copyToLeft: () => runOps(commandOps('copyToLeft'), S.folder.ctxCopyToLeft),
    copyToRight: () => runOps(commandOps('copyToRight'), S.folder.ctxCopyToRight),
    markSelection: toggleSelectionMarks,
  };
  useEffect(
    () =>
      registerFolderView({
        copyToLeft: () => folderActionsRef.current?.copyToLeft(),
        copyToRight: () => folderActionsRef.current?.copyToRight(),
        markSelection: () => folderActionsRef.current?.markSelection(),
      }),
    [],
  );

  // ---- P6b：表头排序（FR-DIR-042）升序→降序→默认 循环（两侧表头共用规格） ----
  const headerSortClick = (key: DirSortKey): void => {
    setSortSpec((prev) => nextSortSpec(prev, key));
  };

  // ---- gutter 复制箭头：单行操作走既有确认 → dir/execute 链 ----
  const runRowOp = (row: DirRow, cmd: OpCommandKind, title: string): void => {
    runOps(buildOpsForCommand(cmd, allRows, new Set([row.id])), title);
  };

  // ---- P6b：基文件夹导航（FR-DIR-003）——面包屑/上一级/交换侧 → 新会话 ----
  const openBaseSession = (newLeft: string, newRight: string): void => {
    if (!newLeft.trim() || !newRight.trim()) {
      setNotice(F.navInvalidPath);
      return;
    }
    openFolderTab(newLeft, newRight);
  };

  // ---- P6b：复制文件名/路径（FR-DIR-066；clipboard 降级提示） ----
  const copyRowText = async (label: string, getText: (row: DirRow) => string): Promise<void> => {
    const { rows, ids } = targetRows();
    const picked = rows.filter((r) => ids.has(r.id));
    if (picked.length === 0) return;
    const text = picked.map(getText).join('\n');
    const outcome = await copyText(text);
    if (outcome === 'failed') {
      // Tauri WebView 剪贴板受限时：内容打印到控制台，提示手动复制
      console.log(`[ai-compare ${label}]\n${text}`);
      setNotice(F.copyFailed);
    } else {
      setNotice(F.copyDone(picked.length));
    }
  };

  const trimSlash = (p: string): string =>
    p.endsWith('/') || p.endsWith('\\') ? p.slice(0, -1) : p;

  // ---- P6b：批量重命名（FR-DIR-064）对话框与执行链 ----
  const openBatchRename = (): void => {
    const { rows, ids } = targetRows();
    const picked = rows.filter((r) => ids.has(r.id));
    if (picked.length === 0) return;
    setBatchRenameState({
      rows: picked,
      allRows,
      onOk: (renames) => {
        setBatchRenameState(null);
        runOps(renamesToOps(renames), F.batchRenameTitle);
      },
      onCancel: () => setBatchRenameState(null),
    });
  };

  // ---- P6b：快速比较（FR-DIR-013）——单行两侧 / 两行互比 ----
  const openQuickCompare = (): void => {
    const { rows, ids } = targetRows();
    const picked = rows.filter((r) => ids.has(r.id));
    const pickedFiles = picked.filter((r) => r.kind !== 'dir');
    const target = pickedFiles.length >= 1 ? pickedFiles.slice(0, 2) : [];
    if (target.length === 0) return;
    setQuickCompareState({ rows: target, onClose: () => setQuickCompareState(null) });
  };

  // ---- R3-16：手动内容比较（FR-DIR-012「操作 > 比较内容」） ----
  const runContentCompare = async (): Promise<void> => {
    const { rows, ids } = targetRows();
    const targets = compareTargetIds(rows, ids);
    const method = effectiveDirCriteria(tab.sessionLink?.sessionId).contentMethod;
    const scopeLabel =
      targets.length > 0 ? F.contentCompareScopeSelected(targets.length) : F.contentCompareScopeAll;
    const onClose = () => setContentCompareState(null);
    if (!contentCompareSupported(getEngineCapabilities())) {
      setContentCompareState({
        method,
        scopeLabel,
        results: null,
        error: F.contentCompareUnsupported,
        onClose,
      });
      return;
    }
    setContentCompareState({ method, scopeLabel, results: null, error: null, onClose });
    try {
      const result = await compareDirContent(
        buildCompareContentParams(cache.sessionId, targets, method),
      );
      // null = 旧引擎无该入口（能力位缺失或 METHOD_NOT_FOUND）：不得当成"已比较"
      setContentCompareState(
        result === null
          ? { method, scopeLabel, results: null, error: F.contentCompareUnsupported, onClose }
          : { method, scopeLabel, results: result.results, error: null, onClose },
      );
    } catch (err: unknown) {
      setContentCompareState({
        method,
        scopeLabel,
        results: null,
        error: F.contentCompareFailed(err instanceof Error ? err.message : String(err)),
        onClose,
      });
    }
  };

  // ---- 同步预设（dir/operate 计划预览 -> 确认执行） ----
  const openPreset = async (preset: SyncPreset): Promise<void> => {
    try {
      const paths = selectedCount > 0 ? [...selection.selected] : undefined;
      const result = await planOperations({
        sessionId: cache.sessionId,
        preset,
        paths,
      });
      setPlanState({
        presetLabel: SYNC_PRESET_LABELS[preset],
        plan: result.plan,
        onConfirm: () => {
          setPlanState(null);
          void doExecute(opsFromPlan(result.plan));
        },
        onCancel: () => setPlanState(null),
      });
    } catch (err: unknown) {
      setNotice(S.folder.opsExecuteFailed(err instanceof Error ? err.message : String(err)));
    }
  };

  // ---- 右键菜单（FR-DIR-060~066 子集） ----
  const buildContextMenu = (rowId: string): ContextMenuItem[] => {
    const { rows } = targetRows();
    const row = rows.find((r) => r.id === rowId);
    const sourceSide: Side | null = row ? (row.left ? 'left' : row.right ? 'right' : null) : null;
    const hasSource = (target: 'left' | 'right') => {
      if (!row) return false;
      if (target === 'left') return row.right != null;
      return row.left != null;
    };
    const sep: ContextMenuItem = { label: '', onSelect: () => {}, separator: true };
    const items: ContextMenuItem[] = [];
    if (row?.kind === 'dir') {
      items.push({
        label: expanded.has(rowId) ? S.folder.collapse : S.folder.expand,
        onSelect: () => toggleDir(row),
      });
      items.push(sep);
    }
    items.push({
      label: S.folder.ctxCopyToRight,
      disabled: !hasSource('right'),
      onSelect: () => runOps(commandOps('copyToRight'), S.folder.ctxCopyToRight),
    });
    items.push({
      label: S.folder.ctxCopyToLeft,
      disabled: !hasSource('left'),
      onSelect: () => runOps(commandOps('copyToLeft'), S.folder.ctxCopyToLeft),
    });
    items.push({
      label: S.folder.ctxMoveToRight,
      disabled: !hasSource('right'),
      onSelect: () => runOps(commandOps('moveToRight'), S.folder.ctxMoveToRight),
    });
    items.push({
      label: S.folder.ctxMoveToLeft,
      disabled: !hasSource('left'),
      onSelect: () => runOps(commandOps('moveToLeft'), S.folder.ctxMoveToLeft),
    });
    items.push({
      label: S.folder.ctxDelete,
      onSelect: () => runOps(commandOps('delete'), S.folder.ctxDelete),
    });
    items.push({
      label: S.folder.ctxTouch,
      disabled: !sourceSide || !row?.[otherSide(sourceSide)],
      onSelect: () => runOps(commandOps('touch'), S.folder.ctxTouch),
    });
    items.push(sep);
    // ---- P6b：快速比较（FR-DIR-013）/ 批量重命名（FR-DIR-064） ----
    const { ids: targetIds } = targetRows();
    const pickedFiles = rows.filter((r) => targetIds.has(r.id) && r.kind !== 'dir');
    items.push({
      label: F.quickCompare,
      disabled: pickedFiles.length === 0 || pickedFiles.length > 2,
      onSelect: () => openQuickCompare(),
    });
    items.push({
      label: F.batchRename,
      disabled: pickedFiles.length === 0 && !row,
      onSelect: () => openBatchRename(),
    });
    items.push({
      label: F.contentCompare,
      disabled: pickedFiles.length === 0,
      onSelect: () => void runContentCompare(),
    });
    items.push(sep);
    // ---- P6b：复制文件名/完整路径/相对路径（FR-DIR-066） ----
    items.push({
      label: F.ctxCopyName,
      disabled: !row,
      onSelect: () => void copyRowText(F.ctxCopyName, (r) => r.name),
    });
    items.push({
      label: F.ctxCopyFullPath,
      disabled: !row,
      onSelect: () =>
        void copyRowText(F.ctxCopyFullPath, (r) => {
          const base = trimSlash(r.left ? folder.left : folder.right);
          return `${base}/${r.id}`;
        }),
    });
    items.push({
      label: F.ctxCopyRelPath,
      disabled: !row,
      onSelect: () => void copyRowText(F.ctxCopyRelPath, (r) => r.id),
    });
    items.push(sep);
    items.push({
      label: S.folder.ctxRename,
      disabled: !row,
      onSelect: () => {
        if (!row) return;
        setRenameState({
          currentName: row.name,
          onOk: (newName) => {
            setRenameState(null);
            runOps(commandOps('rename', { newName }), S.folder.ctxRename);
          },
          onCancel: () => setRenameState(null),
        });
      },
    });
    items.push({ label: S.folder.ctxRefresh, onSelect: () => void applyFilters({ rescan: true }) });
    return items;
  };

  // ---- 键盘：Ctrl/Cmd+A 全选本格（Space 标记走壳级 shortcutEngine → folderViewBus） ----
  const onPaneKeyDown = (side: PaneSide, e: React.KeyboardEvent): void => {
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'a') {
      e.preventDefault();
      setActiveSide(side);
      setSelection(selectAll(visibleIds), side);
    }
  };

  // ---- 单侧窗格：独立表头 + 独立横向滚动 + 独立选择集（FR-DIR-001/003） ----
  const renderPane = (side: PaneSide): React.ReactNode => {
    const scrollRef = side === 'left' ? leftScrollRef : rightScrollRef;
    const headRef = side === 'left' ? leftHeadRef : rightHeadRef;
    const sel = paneSel[side];
    return (
      <div className={`aic-dir-pane${activeSide === side ? ' active' : ''}`} data-side={side}>
        <div className="aic-dir-pane-head" ref={headRef}>
          <div className="aic-dir-grid" style={{ minWidth: PANE_MIN_WIDTH }}>
            {/* P6b：表头排序（FR-DIR-042）——两侧共用排序规格以保持行对齐 */}
            {paneColumns(side).map((col, i) => {
              const active = col.key !== null && sortSpec?.key === col.key;
              const arrow = active ? (sortSpec!.dir === 1 ? ' ▲' : ' ▼') : '';
              const style: React.CSSProperties = col.key
                ? {
                    cursor: 'pointer',
                    userSelect: 'none',
                    color: active ? 'var(--aic-text)' : undefined,
                  }
                : {};
              return (
                <span
                  key={col.label}
                  className={col.align}
                  style={style}
                  title={col.key ? F.sortHint : undefined}
                  onClick={() => col.key && headerSortClick(col.key)}
                >
                  {i === 0 ? `${SIDE_LABEL[side]} · ${col.label}` : col.label}
                  {arrow}
                </span>
              );
            })}
          </div>
        </div>

        <div
          className="aic-dir-scroll"
          ref={scrollRef}
          data-side={side}
          data-version={side === 'left' ? version : undefined}
          tabIndex={0}
          onKeyDown={(e) => onPaneKeyDown(side, e)}
          onFocus={() => setActiveSide(side)}
          onScroll={(e) => onPaneScroll(side, e.currentTarget)}
        >
          <div
            style={{
              height: virtualizer.getTotalSize(),
              position: 'relative',
              minWidth: PANE_MIN_WIDTH,
            }}
          >
            {items.map((item) => {
              const row = visibleRows[item.index];
              const present = row ? sidePresent(row, side) : false;
              const selected = row ? sel.selected.has(row.id) : false;
              return (
                <div
                  key={row?.id ?? item.key}
                  className={`aic-dir-row aic-dir-grid${selected ? ' selected' : ''}${row && !present ? ' missing' : ''}`}
                  data-row-id={row?.id}
                  style={{
                    position: 'absolute',
                    top: 0,
                    left: 0,
                    width: '100%',
                    height: item.size,
                    transform: `translateY(${item.start}px)`,
                  }}
                  onClick={(e) => row && present && rowClick(side, row, e)}
                  onDoubleClick={() => row && present && openRow(row)}
                  onContextMenu={(e) => {
                    e.preventDefault();
                    if (!row || !present) return;
                    setActiveSide(side);
                    if (!sel.selected.has(row.id)) {
                      setSelection(
                        onRowClick(sel, row.id, { ctrl: false, shift: false }, visibleIds),
                        side,
                      );
                    }
                    setCtxMenu({ x: e.clientX, y: e.clientY, rowId: row.id });
                  }}
                >
                  {row && present ? (
                    <PaneRowCells
                      row={row}
                      side={side}
                      depth={rowDepth(row.id)}
                      expanded={expanded.has(row.id)}
                      marked={marks.has(row.id)}
                      onToggleDir={() => toggleDir(row)}
                    />
                  ) : (
                    <>
                      <span className="aic-dir-cell" />
                      <span className="aic-dir-cell r" />
                      <span className="aic-dir-cell r" />
                    </>
                  )}
                </div>
              );
            })}
          </div>
        </div>
      </div>
    );
  };

  // ---- 中央 gutter：内容状态图标 + 左右复制箭头（源侧存在才可点） ----
  const renderGutter = (): React.ReactNode => (
    <div className="aic-dir-gutter">
      <div className="aic-dir-gutter-head" title={S.folder.contentCol}>
        {S.folder.contentCol}
      </div>
      <div
        className="aic-dir-gutter-scroll"
        ref={gutterScrollRef}
        onScroll={(e) => syncVertical('gutter', e.currentTarget)}
      >
        <div style={{ height: virtualizer.getTotalSize(), position: 'relative' }}>
          {items.map((item) => {
            const row = visibleRows[item.index];
            const vis = row ? statusVisual(row.status) : null;
            const dirs = row ? gutterCopyDirs(row) : null;
            return (
              <div
                key={row?.id ?? item.key}
                className="aic-dir-gutter-cell"
                style={{
                  position: 'absolute',
                  top: 0,
                  left: 0,
                  width: '100%',
                  height: item.size,
                  transform: `translateY(${item.start}px)`,
                }}
              >
                {row && vis && dirs && (
                  <>
                    <button
                      className="g-arrow"
                      disabled={!dirs.toLeft}
                      title={S.folder.ctxCopyToLeft}
                      onClick={() => runRowOp(row, 'copyToLeft', S.folder.ctxCopyToLeft)}
                    >
                      ◀
                    </button>
                    <span className={`aic-center-icon ${vis.centerClass}`} title={vis.label}>
                      {vis.centerGlyph}
                    </span>
                    <button
                      className="g-arrow"
                      disabled={!dirs.toRight}
                      title={S.folder.ctxCopyToRight}
                      onClick={() => runRowOp(row, 'copyToRight', S.folder.ctxCopyToRight)}
                    >
                      ▶
                    </button>
                  </>
                )}
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );

  if (error) {
    return (
      <div className="aic-folder">
        <div className="aic-error-box">
          <b>文件夹比较打开失败</b>
          <pre>{error}</pre>
          <button onClick={() => setError(null)}>关闭</button>
        </div>
      </div>
    );
  }

  return (
    <div className="aic-folder">
      <div className="aic-folder-toolbar">
        {/* P6b：基文件夹导航（FR-DIR-003）——可编辑面包屑，点击层级/↑/⇄ 打开新会话 */}
        <BasePathBar left={folder.left} right={folder.right} onOpen={openBaseSession} />
        {ready ? (
          <DirStatsBar snapshot={cache.getStats()} />
        ) : (
          <span style={{ color: 'var(--aic-text-dim)' }}>{S.engine.connecting}</span>
        )}
      </div>

      {/* P6 第二行工具栏：同步预设 / 视图切换 / 显示过滤 / 名称过滤 / 选择 */}
      <div className="aic-folder-toolbar2">
        <span className="dim">{S.folder.syncTitle}</span>
        {SYNC_PRESETS.map((p) => (
          <button
            key={p}
            className="preset"
            disabled={!ready}
            title={SYNC_PRESET_LABELS[p]}
            onClick={() => void openPreset(p)}
          >
            {SYNC_PRESET_LABELS[p]}
          </button>
        ))}
        <span className="sep" />
        <button
          className={`toggle${flat ? '' : ' on'}`}
          disabled={!ready}
          onClick={() => {
            setFlat(false);
            void applyFilters({ flat: false });
          }}
        >
          {S.folder.treeMode}
        </button>
        <button
          className={`toggle${flat ? ' on' : ''}`}
          disabled={!ready}
          onClick={() => {
            setFlat(true);
            void applyFilters({ flat: true });
          }}
        >
          {S.folder.flatMode}
        </button>
        <span className="sep" />
        <select
          className="aic-mini-select"
          value={displayPreset}
          disabled={!ready}
          onChange={(e) => {
            setDisplayPreset(e.target.value);
            void applyFilters({ display: e.target.value });
          }}
        >
          <option value="all">{S.folder.displayAll}</option>
          <option value="diffOnly">{S.folder.displayDiff}</option>
          <option value="sameOnly">{S.folder.displaySame}</option>
          <option value="leftNewer">{S.folder.displayLeftNewer}</option>
          <option value="rightNewer">{S.folder.displayRightNewer}</option>
          <option value="leftOnly">{S.folder.displayLeftOnly}</option>
          <option value="rightOnly">{S.folder.displayRightOnly}</option>
        </select>
        <button
          className={`toggle${suppress ? ' on' : ''}`}
          disabled={!ready}
          title={S.folder.suppressHint}
          onClick={() => {
            const next = !suppress;
            setSuppress(next);
            void applyFilters({ suppress: next });
          }}
        >
          {S.folder.suppress}
        </button>
        <span className="sep" />
        <input
          className="filter-input"
          placeholder={S.folder.filterPlaceholder}
          value={filterText}
          disabled={!ready}
          onChange={(e) => setFilterText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') {
              setAppliedFilterText(filterText.trim());
              void applyFilters({ filterText: filterText.trim() });
            }
          }}
        />
        <button
          className="mini"
          disabled={!ready || !filterText.trim()}
          onClick={() => {
            setAppliedFilterText(filterText.trim());
            void applyFilters({ filterText: filterText.trim() });
          }}
        >
          {S.folder.filterApply}
        </button>
        <button
          className="mini"
          disabled={!ready || (!appliedFilterText && !filterText)}
          onClick={() => {
            setFilterText('');
            setAppliedFilterText('');
            void applyFilters({ filterText: '' });
          }}
        >
          {S.folder.filterClear}
        </button>
        <span className="sep" />
        {/* P6b：编辑菜单（FR-DIR-050/§3.1.6 编辑菜单命令） */}
        <span style={{ position: 'relative' }}>
          <button className="mini" disabled={!ready} onClick={() => setEditMenuOpen((v) => !v)}>
            {F.editMenu} ▾
          </button>
          {editMenuOpen && (
            <>
              <div
                style={{ position: 'fixed', inset: 0, zIndex: 60 }}
                onClick={() => setEditMenuOpen(false)}
              />
              <div
                style={{
                  position: 'absolute',
                  top: 'calc(100% + 2px)',
                  left: 0,
                  zIndex: 61,
                  background: 'var(--aic-bg-panel)',
                  border: '1px solid var(--aic-border)',
                  borderRadius: 6,
                  boxShadow: '0 6px 20px rgba(0,0,0,0.3)',
                  display: 'flex',
                  flexDirection: 'column',
                  minWidth: 220,
                  padding: 4,
                }}
                role="menu"
              >
                {EDIT_MENU_ITEMS.map((mi) =>
                  mi.sep ? (
                    <span
                      key={mi.key}
                      style={{ height: 1, background: 'var(--aic-border)', margin: '3px 0' }}
                    />
                  ) : (
                    <button
                      key={mi.key}
                      role="menuitem"
                      style={editMenuItemStyle}
                      onClick={() => applyEditCommand(mi.cmd!, mi.label)}
                    >
                      {mi.label}
                    </button>
                  ),
                )}
                <span style={{ height: 1, background: 'var(--aic-border)', margin: '3px 0' }} />
                <button
                  style={editMenuItemStyle}
                  onClick={() => {
                    setSelection(selectAll(visibleIds));
                    setEditMenuOpen(false);
                  }}
                >
                  {S.folder.selectAll}
                </button>
                <button
                  style={editMenuItemStyle}
                  onClick={() => {
                    setSelection(invertSelection(selection, visibleIds));
                    setEditMenuOpen(false);
                  }}
                >
                  {S.folder.invertSel}
                </button>
                <button
                  style={editMenuItemStyle}
                  onClick={() => {
                    setSelection(clearSelection());
                    setEditMenuOpen(false);
                  }}
                >
                  {S.folder.clearSel}
                </button>
              </div>
            </>
          )}
        </span>
        {/* R3-16：手动内容比较（FR-DIR-012「操作 > 比较内容」），方法取自生效设置 */}
        <button
          className="mini"
          disabled={!ready}
          title={F.contentCompareHint}
          onClick={() => void runContentCompare()}
        >
          {F.contentCompare}
        </button>
        <button
          className="mini"
          disabled={!ready}
          onClick={() => setSelection(selectAll(visibleIds))}
        >
          {S.folder.selectAll}
        </button>
        <button
          className="mini"
          disabled={!ready}
          onClick={() => setSelection(invertSelection(selection, visibleIds))}
        >
          {S.folder.invertSel}
        </button>
        <button className="mini" disabled={!ready} onClick={() => setSelection(clearSelection())}>
          {S.folder.clearSel}
        </button>
        {/* P6b：空格标记 —— 仅显示标记过滤 + 标记计数 */}
        <span className="sep" />
        <button
          className={`toggle${markOnly ? ' on' : ''}`}
          disabled={!ready}
          title={F.markOnlyHint}
          onClick={() => setMarkOnly((v) => !v)}
        >
          {F.markOnly}
        </button>
        {markCount > 0 && (
          <span className="sel-chip" style={{ color: MARK_ORANGE }} title={F.markHint}>
            {F.markedCount(markCount)}
          </span>
        )}
        {(['left', 'right'] as PaneSide[]).map((side) =>
          paneSel[side].selected.size > 0 ? (
            <span
              key={side}
              className="sel-chip"
              style={{ opacity: activeSide === side ? 1 : 0.5 }}
              title={
                activeSide === side
                  ? F.paneActive(SIDE_LABEL[side])
                  : F.paneInactive(SIDE_LABEL[side])
              }
            >
              {F.paneSelChip(SIDE_LABEL[side], paneSel[side].selected.size)}
            </span>
          ) : null,
        )}
        {!flat && (
          <>
            <span className="sep" />
            <button className="mini" onClick={() => setExpanded(expandAll(allRows))}>
              {S.folder.expandAll}
            </button>
            <button className="mini" onClick={() => setExpanded(collapseAll())}>
              {S.folder.collapseAll}
            </button>
          </>
        )}
      </div>

      {/* 操作结果反馈条（OP-002 部分成功语义） */}
      {running != null && (
        <div className="aic-ops-result">
          <span className="aic-progress">
            <span className="bar">
              <i />
            </span>
            {S.folder.opsRunning(running)}
          </span>
        </div>
      )}
      {execReport && running == null && (
        <>
          <div className={`aic-ops-result${execReport.failed > 0 ? ' has-failed' : ''}`}>
            <span>{S.folder.opsDone(execReport.ok, execReport.failed, execReport.skipped)}</span>
            <button className="detail-toggle" onClick={() => setShowDetail((v) => !v)}>
              {S.folder.opsDetail}
            </button>
            <button className="close" onClick={() => setExecReport(null)}>
              {S.folder.opsClose}
            </button>
          </div>
          {showDetail && (
            <div className="aic-ops-detail">
              {execReport.details.map((d, i) => (
                <div key={`${d.path}-${i}`}>
                  <span className={`st-${d.status}`}>{d.status}</span>{' '}
                  {S.folder.opsDetailItem('', d.kind, d.path, d.detail).replace('[undefined] ', '')}
                </div>
              ))}
            </div>
          )}
        </>
      )}

      {/* BC 双窗格 + 中央 gutter（FR-DIR-001）：行按名称对齐，纵向滚动三列同步 */}
      <div className="aic-dir-body">
        {renderPane('left')}
        {renderGutter()}
        {renderPane('right')}
      </div>

      {ctxMenu && (
        <ContextMenu
          x={ctxMenu.x}
          y={ctxMenu.y}
          items={buildContextMenu(ctxMenu.rowId)}
          onClose={() => setCtxMenu(null)}
        />
      )}
      {confirmState && <ConfirmOpsDialog state={confirmState} />}
      {planState && <PlanPreviewDialog state={planState} />}
      {renameState && <RenameDialog state={renameState} />}
      {batchRenameState && <BatchRenameDialog state={batchRenameState} />}
      {quickCompareState && <QuickCompareDialog state={quickCompareState} />}
      {contentCompareState && <ContentCompareDialog state={contentCompareState} />}
    </div>
  );

  function openRow(row: DirRow): void {
    if (row.kind === 'dir') {
      toggleDir(row);
      return;
    }
    if (row.status === 'leftOnly' || row.status === 'rightOnly') {
      setNotice(S.folder.orphanHint);
      return;
    }
    void openChild(tab.id, row);
  }
}

/**
 * 单侧行渲染（P6 + P6b + 双窗格）：目录展开箭头 + 名称缩进 + 抑制青绿
 * （FR-DIR-035）+ 标记橙色角标（P6b 空格标记；与抑制徽标并存）。
 * 只渲染本侧的 名称/大小/已修改 三列；缺侧由调用方整行留白。
 * suppressed 行整行青绿色 + 徽标；hidden 行不进入 visibleRows（dirTree 过滤）。
 */
function PaneRowCells({
  row,
  side,
  depth,
  expanded,
  marked,
  onToggleDir,
}: {
  row: DirRow;
  side: PaneSide;
  depth: number;
  expanded: boolean;
  marked: boolean;
  onToggleDir(): void;
}) {
  const vis = statusVisual(row.status);
  const info = row[side];
  const isDir = row.kind === 'dir';
  const isSuppressed = row.suppressed;
  const nameClass = isSuppressed ? 'aic-status-filtered' : toneClass(vis.tone);
  const dotClass = isSuppressed ? 'filtered' : statusDotClass[vis.tone];
  return (
    <>
      <span
        className={`aic-dir-cell name ${nameClass}`}
        title={`${SIDE_LABEL[side]} ${row.id} · ${marked ? `${F.markedBadge} · ` : ''}${isSuppressed ? `${S.folder.suppressedBadge} · ` : ''}${vis.label}`}
        style={{ paddingLeft: 12 + depth * 18 }}
      >
        <span
          className={`aic-dir-caret${isDir ? '' : ' empty'}`}
          onClick={(e) => {
            e.stopPropagation();
            onToggleDir();
          }}
        >
          {isDir ? (expanded ? '▾' : '▸') : ''}
        </span>
        <span className="aic-dir-name">
          <i className={`aic-dot ${dotClass}`} />
          <span className="fname">{row.name}</span>
          {marked && <span style={markBadgeStyle}>{F.markedBadge}</span>}
          {isSuppressed && <span className="sup-badge">{S.folder.suppressedBadge}</span>}
        </span>
      </span>
      <span className="aic-dir-cell mono r">{formatSize(info?.size)}</span>
      <span className="aic-dir-cell mono r">{formatTime(info?.mtimeMs)}</span>
    </>
  );
}
