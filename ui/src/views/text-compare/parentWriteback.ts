import type { DirFilterParams } from '../../protocol/types';

/**
 * 子会话保存 → 父目录行回传（FR-SES-004 / R3-17）。
 *
 * BC 语义：在子会话里改完文件保存后，父文件夹会话中该行的状态与元数据必须追平，
 * 否则用户回到父视图看到的仍是保存前的旧判定。回传走父会话既有的重扫入口
 * （dir/filter rescan:true，与右键"刷新"同一条路径），引擎重扫后推 dir/delta，
 * 父会话缓存是会话级订阅（切换标签不解除），因此父标签即使不在前台也会追平。
 *
 * 独立文本比较（无父会话）不得触发任何重扫。
 */
export function parentRescanParams(
  parentFolderSessionId: string | null | undefined,
): DirFilterParams | null {
  if (!parentFolderSessionId) return null;
  // 过滤器/显示/平铺一律不传 = 不改动父会话现有设置（引擎按 is_some 门控），
  // 只要求重扫；传当前值反而会把子会话无从得知的父设置覆盖掉。
  return {
    sessionId: parentFolderSessionId,
    suppress: undefined,
    flat: undefined,
    rescan: true,
  };
}
