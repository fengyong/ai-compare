import { invoke } from '@tauri-apps/api/core';
import { isTauriRuntime } from './env';

/**
 * 剪贴板比较（docs/01 §3.12 FR-CLIP-001~004）。
 *
 * - `clipboard://N` 是 BC 的剪贴板规格（docs/01 §2.5）：N 为本应用剪贴板
 *   历史条目 id，历史由本模块维护（去重/最小长度/条数与内存上限）。
 * - 浏览器 dev（mock 引擎）：规格原样传给 text/open，MockEngine 直接从本
 *   模块读回文本，演示数据即真实剪贴板内容。
 * - Tauri 桌面（真实引擎）：VFS 只认本地路径，故用既有 `settings_write`
 *   Tauri command 把条目物化到设置目录下的临时 .txt，再把绝对路径交给
 *   text/open；无需改 Rust 协议面。
 */

export interface ClipboardEntry {
  id: number;
  text: string;
  ts: number;
}

/** FR-CLIP-003：最小剪辑长度、条数上限、内存上限（即使超限也保留最近两条） */
export interface ClipboardLimits {
  minLength: number;
  maxEntries: number;
  maxBytes: number;
}

export const DEFAULT_CLIP_LIMITS: ClipboardLimits = {
  minLength: 0,
  maxEntries: 20,
  maxBytes: 2 * 1024 * 1024,
};

export function clipByteLength(text: string): number {
  return new TextEncoder().encode(text).length;
}

export function clipTotalBytes(history: readonly ClipboardEntry[]): number {
  return history.reduce((n, e) => n + clipByteLength(e.text), 0);
}

/** 追加一条剪贴板文本：空/过短/与上一条相同 → 不新增；随后按上限裁剪 */
export function appendClip(
  history: readonly ClipboardEntry[],
  text: string,
  ts: number,
  id: number,
  limits: ClipboardLimits = DEFAULT_CLIP_LIMITS,
): ClipboardEntry[] {
  if (text.length === 0 || text.length < limits.minLength) return [...history];
  if (history.length > 0 && history[history.length - 1].text === text) return [...history];
  const next: ClipboardEntry[] = [...history, { id, text, ts }];
  let out = next.slice(-Math.max(2, limits.maxEntries));
  while (out.length > 2 && clipTotalBytes(out) > limits.maxBytes) out = out.slice(1);
  return out;
}

/** 最近两条（较旧在左、最新在右，对应 BC 的新文本比较窗格） */
export function latestTwo(
  history: readonly ClipboardEntry[],
): [ClipboardEntry, ClipboardEntry] | null {
  if (history.length < 2) return null;
  return [history[history.length - 2], history[history.length - 1]];
}

export function clipLabel(entry: ClipboardEntry, max = 48): string {
  const first = entry.text.split(/\r?\n/).find((l) => l.trim().length > 0) ?? '';
  const clean = first.trim() || '（空白）';
  return clean.length > max ? `${clean.slice(0, max)}…` : clean;
}

/** `clipboard://12` → 12；普通路径 → null */
export function parseClipSpec(path: string): number | null {
  const m = /^clipboard:\/\/(\d+)$/.exec(path);
  return m ? Number(m[1]) : null;
}

interface AppDirsResult {
  base_dir: string;
}

/** 单例：历史 + 订阅 + Tauri 物化 + 另存为 */
export class ClipboardStore {
  private entries: ClipboardEntry[] = [];
  private seq = 0;
  private readonly listeners = new Set<() => void>();
  private limits: ClipboardLimits = DEFAULT_CLIP_LIMITS;

  constructor(private readonly storageKey = 'ai-compare.clipboard.v1') {
    this.load();
  }

  list(): ClipboardEntry[] {
    return [...this.entries];
  }

  getText(id: number): string | undefined {
    return this.entries.find((e) => e.id === id)?.text;
  }

  setLimits(limits: Partial<ClipboardLimits>): void {
    this.limits = { ...this.limits, ...limits };
  }

  subscribe(fn: () => void): () => void {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }

  private emit(): void {
    this.persist();
    for (const fn of this.listeners) fn();
  }

  /** 返回新增条目（被过滤时返回 null） */
  add(text: string, ts = Date.now()): ClipboardEntry | null {
    const id = ++this.seq;
    this.entries = appendClip(this.entries, text, ts, id, this.limits);
    const tail = this.entries[this.entries.length - 1];
    // 用「尾部是否为本轮新 id」判断，而非长度相等：历史已满时裁剪会
    // 保持长度不变，但新条目确实已写入（原实现会误返回 null）。
    if (!tail || tail.id !== id) return null;
    this.emit();
    return tail;
  }

  /** 注册一段即时文本（如编辑器选择），返回可传给 text/open 的规格 */
  registerText(text: string): string {
    const last = this.entries[this.entries.length - 1];
    if (last?.text === text) return `clipboard://${last.id}`;
    // add 可能因空文本/最小长度/相邻去重被过滤；此处必须强制插入，
    // 否则空历史场景会返回无效的 clipboard://0，text/open 找不到条目。
    const id = ++this.seq;
    // 直接插入（绕过 appendClip 的空文本/最小长度过滤）：返回的规格必须
    // 始终指向真实条目；裁剪只丢最旧项，新条目一定保留在尾部。
    const next = [...this.entries, { id, text, ts: Date.now() }];
    this.entries = next.slice(-Math.max(2, this.limits.maxEntries));
    this.emit();
    return `clipboard://${id}`;
  }

  remove(id: number): void {
    this.entries = this.entries.filter((e) => e.id !== id);
    this.emit();
  }

  clear(): void {
    this.entries = [];
    this.emit();
  }

  /** 物化为引擎可读路径（Tauri 写临时文件；浏览器保留 clipboard:// 规格） */
  async materialize(path: string): Promise<string> {
    const id = parseClipSpec(path);
    if (id === null) return path;
    const text = this.getText(id);
    if (text === undefined) throw new Error(`剪贴板条目不存在或已被删除: ${path}`);
    if (!isTauriRuntime()) return path;
    const dirs = await invoke<AppDirsResult>('settings_app_dirs');
    const base = dirs.base_dir.replace(/[\\/]+$/, '');
    const file = `${base}/clipboard/clip-${id}.txt`;
    await invoke('settings_write', { path: file, contents: text });
    return file;
  }

  /** FR-CLIP-001 右键「另存为…」：导出为 .txt 下载 */
  saveAs(id: number): void {
    const text = this.getText(id);
    if (text === undefined || typeof document === 'undefined') return;
    const blob = new Blob([text], { type: 'text/plain;charset=utf-8' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = `clipboard-${id}.txt`;
    a.click();
    URL.revokeObjectURL(url);
  }

  private load(): void {
    try {
      if (typeof localStorage === 'undefined') return;
      const raw = localStorage.getItem(this.storageKey);
      if (!raw) return;
      const parsed = JSON.parse(raw) as ClipboardEntry[];
      if (!Array.isArray(parsed)) return;
      this.entries = parsed.filter(
        (e) =>
          typeof e?.id === 'number' && typeof e?.text === 'string' && typeof e?.ts === 'number',
      );
      this.seq = this.entries.reduce((m, e) => Math.max(m, e.id), 0);
    } catch {
      this.entries = [];
    }
  }

  private persist(): void {
    try {
      if (typeof localStorage === 'undefined') return;
      localStorage.setItem(this.storageKey, JSON.stringify(this.entries));
    } catch {
      // 配额/隐私模式：历史仍保留在内存，不阻断比较
    }
  }
}

export const clipboardStore = new ClipboardStore();
