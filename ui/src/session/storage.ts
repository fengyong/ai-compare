import { ParseError } from './model';

/**
 * 持久化纯函数层（SET-001 / ADR-005：版本化单一 JSON 文档 + 原子写 + .bak 回滚）。
 *
 * 原子写协议（各 IO 后端遵守同一语义，Rust/Tauri 侧为真 rename 原子写）：
 *   1. 若目标已存在 → 先复制为 `<name>.bak`（写前备份，BC 的 BCSessions.xml.bak 惯例）
 *   2. 写入临时文件 `<name>.tmp-<rand>` 并 fsync（后端负责）
 *   3. rename 临时文件 → 目标（原子替换；失败时目标与 .bak 均保持旧值）
 *
 * 读取恢复协议（R12 演练：损坏 → 自动回滚 .bak 并提示）：
 *   - 主文件可解析 → 正常使用；
 *   - 主文件损坏且 .bak 可解析 → 采用 .bak 并上报 recoveredFromBackup（UI 提示 + 立即回写修复）；
 *   - 两者都不可用 → 上报 lost（新空文档起步，提示用户）。
 */

export const SETTINGS_FILES = {
  sessions: 'sessions.json',
  kindDefaults: 'session-defaults.json',
  settings: 'settings.json',
  state: 'state.json',
} as const;

/** 原子写计划（纯描述，便于单测与各后端对齐执行） */
export interface AtomicWritePlan {
  /** 需要写入的 .bak 内容（目标旧值）；目标不存在时为 null */
  backupContent: string | null;
  backupPathSuffix: '.bak';
  tmpPathSuffix: string;
  /** 最终落盘内容 */
  finalContent: string;
}

/** 生成原子写计划：existing 为当前目标文件内容（null = 不存在） */
export function createAtomicWritePlan(existing: string | null, incoming: string): AtomicWritePlan {
  return {
    backupContent: existing === incoming ? null : existing,
    backupPathSuffix: '.bak',
    tmpPathSuffix: `.tmp-${Math.random().toString(36).slice(2, 8)}`,
    finalContent: incoming,
  };
}

export type RecoverySource = 'primary' | 'backup' | 'lost';

export interface RecoveryResult<T> {
  doc: T | null;
  source: RecoverySource;
  /** 主文件损坏原因（source=backup/lost 时有值，供 UI 提示） */
  primaryError?: string;
  /** .bak 也损坏的原因（source=lost 时有值） */
  backupError?: string;
}

/**
 * 双源恢复：主文件 → .bak → 失败。
 * parse 抛出（含 ParseError）即视为该源不可用。
 */
export function recoverWithBackup<T>(
  primary: string | null,
  backup: string | null,
  parse: (text: string) => T,
): RecoveryResult<T> {
  if (primary !== null) {
    try {
      return { doc: parse(primary), source: 'primary' };
    } catch (err) {
      const primaryError =
        err instanceof ParseError || err instanceof Error ? err.message : String(err);
      if (backup !== null) {
        try {
          return { doc: parse(backup), source: 'backup', primaryError };
        } catch (err2) {
          return {
            doc: null,
            source: 'lost',
            primaryError,
            backupError: err2 instanceof Error ? err2.message : String(err2),
          };
        }
      }
      return { doc: null, source: 'lost', primaryError };
    }
  }
  // 主文件不存在：.bak 存在属异常状态（上次原子写中断），仍尝试恢复
  if (backup !== null) {
    try {
      return {
        doc: parse(backup),
        source: 'backup',
        primaryError: '主文件缺失（上次写入可能被中断）',
      };
    } catch (err) {
      return {
        doc: null,
        source: 'lost',
        primaryError: '主文件缺失',
        backupError: err instanceof Error ? err.message : String(err),
      };
    }
  }
  return { doc: null, source: 'primary' }; // 全新安装：主/备均无，不算损坏
}

/** MRU 去重 + 截断（新条目置前） */
export function pushMru(list: string[], item: string, max = 10): string[] {
  const next = [item, ...list.filter((x) => x !== item)];
  return next.slice(0, max);
}

/** MRU 路径规整：去首尾空白；相对路径保持原样（便携模式相对存储的口径） */
export function normalizePathInput(p: string): string {
  return p.trim();
}
