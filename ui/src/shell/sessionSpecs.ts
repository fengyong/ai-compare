import type { SessionKind, SessionSpecs } from '../session/model';
import type { Tab } from './shellState';

/**
 * 标签 → 命名会话（SES-001 保存/另存/重开）。
 *
 * 纯函数便于逐类型断言：三路会话必须落 base/left/right 三个路径位，
 * 图片/表格/十六进制必须落真实路径与真实类型（R3-08）。
 */

/** 标签类型 → 会话类型（一一对应；home 无会话语义，按目录比较兜底） */
export function sessionKindOfTab(kind: Tab['kind']): SessionKind {
  switch (kind) {
    case 'text':
      return 'text-compare';
    case 'edit':
      return 'text-edit';
    case 'sync':
      return 'folder-sync';
    case 'merge':
      return 'folder-merge';
    case 'merge3':
      return 'text-merge';
    case 'picture':
      return 'picture-compare';
    case 'table':
      return 'table-compare';
    case 'hex':
      return 'hex-compare';
    default:
      return 'folder-compare';
  }
}

/** 标签 → 会话规格（保存用；缺数据的标签落空串，由保存入口校验） */
export function specsOfTab(tab: Tab): SessionSpecs {
  if (tab.kind === 'folder' && tab.folder)
    return { left: tab.folder.left, right: tab.folder.right };
  if (tab.kind === 'sync' && tab.sync) return { left: tab.sync.left, right: tab.sync.right };
  if (tab.kind === 'picture' && tab.picture)
    return { left: tab.picture.left, right: tab.picture.right };
  if (tab.kind === 'table' && tab.table) return { left: tab.table.left, right: tab.table.right };
  if (tab.kind === 'hex' && tab.hex) return { left: tab.hex.left, right: tab.hex.right };
  if (tab.kind === 'merge' && tab.merge)
    return { base: tab.merge.base, left: tab.merge.left, right: tab.merge.right };
  if (tab.kind === 'merge3' && tab.merge3)
    return { base: tab.merge3.base, left: tab.merge3.left, right: tab.merge3.right };
  if (tab.kind === 'text' && tab.text)
    return { left: tab.text.left.path, right: tab.text.right.path };
  // D2：单文件会话只有左位；right 落空串（specs 归一化允许）
  if (tab.kind === 'edit' && tab.edit) return { left: tab.edit.path, right: '' };
  return { left: '', right: '' };
}

/** 三路会话的 base：用户显式填写优先，留空时以 left 兼作（BC 允许 base 缺省） */
export function resolveMergeBase(base: string, left: string): string {
  return base.trim() || left.trim();
}
