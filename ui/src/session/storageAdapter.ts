import { invoke } from '@tauri-apps/api/core';
import { isTauriRuntime } from '../lib/env';
import { createAtomicWritePlan, type AtomicWritePlan, SETTINGS_FILES } from './storage';

/**
 * 设置文件 IO 适配层（SET-001）。
 *
 * 统一 KvFileStore 接口，三种后端：
 * - MemoryFileStore    单测/回放（无副作用）
 * - BrowserFileStore   浏览器 dev：localStorage（原子写语义以"先 .bak 后主键"近似）
 * - TauriFileStore     桌面：invoke settings_read/settings_write（Rust 侧真 tmp+rename 原子写）
 *
 * 存储位置与便携模式（FR-SET-001）：
 * - 桌面默认：平台设置目录（macOS ~/Library/Application Support/ai-compare 等）；
 * - 便携模式：可执行文件旁存在 portable.ini（或 ai-compare.ini）→ 设置存 exe 同目录（相对路径存储）；
 * - 浏览器：localStorage 键名 `aic:<文件名>`（虚拟路径），无便携概念。
 */

export interface KvFileStore {
  readFile(path: string): Promise<string | null>;
  writeFile(path: string, contents: string): Promise<void>;
  deleteFile(path: string): Promise<void>;
}

export interface AppDirs {
  /** 设置根目录（便携模式下 = exe 目录） */
  baseDir: string;
  exeDir: string;
  portable: boolean;
}

interface AppDirsCommandResult {
  base_dir: string;
  exe_dir: string;
  portable: boolean;
}

let dirsCache: AppDirs | null = null;

/** 探测设置目录与便携模式（Tauri 内 invoke 命令；浏览器返回 null） */
export async function resolveAppDirs(): Promise<AppDirs | null> {
  if (!isTauriRuntime()) return null;
  if (dirsCache) return dirsCache;
  const r = await invoke<AppDirsCommandResult>('settings_app_dirs');
  dirsCache = { baseDir: r.base_dir, exeDir: r.exe_dir, portable: r.portable };
  return dirsCache;
}

/** 供测试重置缓存 */
export function resetAppDirsCacheForTest(): void {
  dirsCache = null;
}

export function joinSettingPath(dir: string, file: string): string {
  return dir.endsWith('/') || dir.endsWith('\\') ? `${dir}${file}` : `${dir}/${file}`;
}

/** 四个设置文件的最终路径（浏览器返回 null → 调用方走 localStorage 键名） */
export async function resolveSettingPaths(): Promise<Record<keyof typeof SETTINGS_FILES, string> | null> {
  const dirs = await resolveAppDirs();
  if (!dirs) return null;
  return {
    sessions: joinSettingPath(dirs.baseDir, SETTINGS_FILES.sessions),
    kindDefaults: joinSettingPath(dirs.baseDir, SETTINGS_FILES.kindDefaults),
    settings: joinSettingPath(dirs.baseDir, SETTINGS_FILES.settings),
    state: joinSettingPath(dirs.baseDir, SETTINGS_FILES.state),
  };
}

// ------------------------------------------------------------------
// Memory
// ------------------------------------------------------------------

export class MemoryFileStore implements KvFileStore {
  private files = new Map<string, string>();

  async readFile(path: string): Promise<string | null> {
    return this.files.get(path) ?? null;
  }

  async writeFile(path: string, contents: string): Promise<void> {
    const plan = createAtomicWritePlan(this.files.get(path) ?? null, contents);
    this.applyPlan(path, plan);
  }

  async deleteFile(path: string): Promise<void> {
    this.files.delete(path);
    // .bak 必须一起删：否则删除后走恢复协议会把旧文档复活（R3-13）
    this.files.delete(`${path}.bak`);
  }

  /** 按 AtomicWritePlan 语义应用（测试可断言 .bak/.tmp 的中间状态） */
  applyPlan(path: string, plan: AtomicWritePlan): void {
    const tmp = `${path}${plan.tmpPathSuffix}`;
    this.files.set(tmp, plan.finalContent);
    if (plan.backupContent !== null) {
      this.files.set(`${path}.bak`, plan.backupContent);
    }
    this.files.set(path, this.files.get(tmp)!);
    this.files.delete(tmp);
  }

  /** 测试辅助：直读内部表 */
  peek(path: string): string | null {
    return this.files.get(path) ?? null;
  }
}

// ------------------------------------------------------------------
// Browser（localStorage）
// ------------------------------------------------------------------

const LS_PREFIX = 'aic:';

export class BrowserFileStore implements KvFileStore {
  constructor(private readonly storage: Storage | null = typeof localStorage === 'undefined' ? null : localStorage) {}

  private key(path: string): string {
    // 浏览器模式下 path 为虚拟文件名（sessions.json 等）；带路径也统一扁平化
    return LS_PREFIX + path.replace(/[\\/]/g, '_');
  }

  async readFile(path: string): Promise<string | null> {
    return this.storage?.getItem(this.key(path)) ?? null;
  }

  async writeFile(path: string, contents: string): Promise<void> {
    if (!this.storage) return;
    const plan = createAtomicWritePlan(this.storage.getItem(this.key(path)), contents);
    // 近似原子序：先落 .bak（旧值），再覆盖主键；localStorage 单键写入本身原子
    if (plan.backupContent !== null) {
      this.storage.setItem(`${this.key(path)}.bak`, plan.backupContent);
    }
    this.storage.setItem(this.key(path), plan.finalContent);
  }

  async deleteFile(path: string): Promise<void> {
    this.storage?.removeItem(this.key(path));
    // 与 Memory/Tauri 后端同口径：.bak 一并删除，避免恢复协议复活旧文档
    this.storage?.removeItem(`${this.key(path)}.bak`);
  }
}

// ------------------------------------------------------------------
// Tauri（Rust 壳原子写）
// ------------------------------------------------------------------

export class TauriFileStore implements KvFileStore {
  async readFile(path: string): Promise<string | null> {
    const r = await invoke<{ found: boolean; contents: string | null }>('settings_read', { path });
    return r.found ? (r.contents ?? '') : null;
  }

  async writeFile(path: string, contents: string): Promise<void> {
    await invoke('settings_write', { path, contents });
  }

  async deleteFile(path: string): Promise<void> {
    await invoke('settings_delete', { path });
  }
}

/** 按运行环境选择后端（Tauri → 真文件；浏览器 → localStorage） */
export function createDefaultFileStore(): KvFileStore {
  return isTauriRuntime() ? new TauriFileStore() : new BrowserFileStore();
}
