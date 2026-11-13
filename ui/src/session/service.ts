import {
  emptyGlobalSettingsDoc,
  emptyKindDefaultsDoc,
  emptySessionsDoc,
  emptyStateDoc,
  newSessionId,
  parseGlobalSettingsDoc,
  parseKindDefaultsDoc,
  parseSessionsDoc,
  parseStateDoc,
  serializeDoc,
  SESSION_KINDS,
  type GlobalSettingsDoc,
  type KindDefaultsDoc,
  type SessionKind,
  type SessionNode,
  type SessionSpecs,
  type SessionState,
  type SessionsDoc,
  type SettingsScope,
  type SettingsTree,
  type StateDoc,
} from './model';
import { applyPatchToLayer, resolveEffective, type EffectiveLayer } from './scopes';
import {
  normalizePathInput,
  pushMru,
  recoverWithBackup,
  SETTINGS_FILES,
  type RecoveryResult,
} from './storage';
import { createDefaultFileStore, resolveSettingPaths, type KvFileStore } from './storageAdapter';

/**
 * 会话服务（M06 会话层的 UI 侧落地，docs/03 §3.3）：
 * 会话树 CRUD / 锁定 / MRU / 四层设置的读写与作用域写回 / 四文件的加载与恢复。
 *
 * - init() 完成四个设置文件的加载（损坏 → .bak 回滚 → 恢复报告供 UI 提示）；
 * - 所有写操作更新内存文档后立即落盘（原子写 + .bak，见 storage.ts）；
 * - 变更通过 subscribe 广播（主页会话树/设置对话框订阅刷新）。
 */

export interface InitReport {
  /** 从 .bak 自动恢复的文件（UI 提示"已从备份恢复"） */
  recoveredFromBackup: string[];
  /** 主/备均损坏、已以空文档起步的文件（UI 提示自动恢复失败） */
  lost: Array<{ file: string; primaryError?: string; backupError?: string }>;
  /** 桌面便携模式（可执行文件旁 portable.ini） */
  portable: boolean;
  /** 设置根目录（浏览器为 null） */
  baseDir: string | null;
}

export interface SessionTreeVM {
  rootFolderIds: string[];
  folders: Map<string, { id: string; name: string; children: string[] }>;
  sessions: Map<string, SessionNode>;
}

export interface SaveSessionInput {
  kind: SessionKind;
  name: string;
  specs: SessionSpecs;
  overrides?: SettingsTree;
  folderId?: string | null;
  state?: SessionState;
}

export class SessionService {
  private sessionsDoc: SessionsDoc = emptySessionsDoc();
  private kindDefaults: KindDefaultsDoc = emptyKindDefaultsDoc();
  private globalSettings: GlobalSettingsDoc = emptyGlobalSettingsDoc();
  private stateDoc: StateDoc = emptyStateDoc();
  private paths: Record<keyof typeof SETTINGS_FILES, string> | null = null;
  private readonly listeners = new Set<() => void>();

  readonly report: InitReport = { recoveredFromBackup: [], lost: [], portable: false, baseDir: null };

  constructor(private readonly store: KvFileStore = createDefaultFileStore()) {}

  // ---------------- 订阅 ----------------

  subscribe(fn: () => void): () => void {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }

  private emit(): void {
    for (const fn of this.listeners) fn();
  }

  // ---------------- 初始化 ----------------

  async init(): Promise<InitReport> {
    this.paths = await resolveSettingPaths();
    if (this.paths) {
      this.report.baseDir = this.paths.sessions.slice(0, -SETTINGS_FILES.sessions.length - 1);
    }

    const recoveredKeys: Array<keyof typeof SETTINGS_FILES> = [];
    await this.loadDoc('sessions', parseSessionsDoc, (doc) => {
      this.sessionsDoc = doc as SessionsDoc;
    }, emptySessionsDoc, recoveredKeys);
    await this.loadDoc('kindDefaults', parseKindDefaultsDoc, (doc) => {
      this.kindDefaults = doc as KindDefaultsDoc;
    }, emptyKindDefaultsDoc, recoveredKeys);
    await this.loadDoc('settings', parseGlobalSettingsDoc, (doc) => {
      this.globalSettings = doc as GlobalSettingsDoc;
    }, emptyGlobalSettingsDoc, recoveredKeys);
    await this.loadDoc('state', parseStateDoc, (doc) => {
      this.stateDoc = doc as StateDoc;
    }, emptyStateDoc, recoveredKeys);

    // 恢复路径：主文件损坏但从 .bak 恢复 → 立即回写主文件（修复），下次直读主文件
    for (const key of recoveredKeys) {
      await this.persistByFileKey(key);
    }
    return this.report;
  }

  private async loadDoc<T>(
    fileKey: keyof typeof SETTINGS_FILES,
    parse: (text: string) => T,
    assign: (doc: T) => void,
    empty: () => T,
    recoveredKeys?: Array<keyof typeof SETTINGS_FILES>
  ): Promise<void> {
    const path = this.pathOf(fileKey);
    const [primary, backup] = await Promise.all([this.store.readFile(path), this.store.readFile(`${path}.bak`)]);
    const r: RecoveryResult<T> = recoverWithBackup(primary, backup, parse);
    if (r.source === 'primary' && r.doc !== null) {
      assign(r.doc);
      return;
    }
    if (r.source === 'backup' && r.doc !== null) {
      assign(r.doc);
      this.report.recoveredFromBackup.push(SETTINGS_FILES[fileKey]);
      recoveredKeys?.push(fileKey);
      return;
    }
    if (r.source === 'lost') {
      assign(empty());
      this.report.lost.push({ file: SETTINGS_FILES[fileKey], primaryError: r.primaryError, backupError: r.backupError });
      return;
    }
    // 全新安装：主/备均无
    assign(empty());
  }

  /** 文件键 → 实际路径（桌面：设置目录绝对路径；浏览器/内存：文件名本身） */
  private pathOf(fileKey: keyof typeof SETTINGS_FILES): string {
    return this.paths?.[fileKey] ?? SETTINGS_FILES[fileKey];
  }

  private async persist(doc: 'sessions' | 'kindDefaults' | 'settings' | 'state'): Promise<void> {
    await this.persistByFileKey(doc);
  }

  private async persistByFileKey(fileKey: keyof typeof SETTINGS_FILES): Promise<void> {
    const path = this.pathOf(fileKey);
    const content =
      fileKey === 'sessions'
        ? serializeDoc(this.sessionsDoc)
        : fileKey === 'kindDefaults'
          ? serializeDoc(this.kindDefaults)
          : fileKey === 'settings'
            ? serializeDoc(this.globalSettings)
            : serializeDoc(this.stateDoc);
    await this.store.writeFile(path, content);
  }

  // ---------------- 会话树 ----------------

  getTree(): SessionTreeVM {
    const folders = new Map<string, { id: string; name: string; children: string[] }>();
    for (const f of this.sessionsDoc.folders) folders.set(f.id, { ...f, children: [...f.children] });
    const sessions = new Map<string, SessionNode>();
    for (const s of this.sessionsDoc.sessions) sessions.set(s.id, { ...s });
    return { rootFolderIds: [...this.sessionsDoc.rootFolderIds], folders, sessions };
  }

  getSession(id: string): SessionNode | undefined {
    const s = this.sessionsDoc.sessions.find((x) => x.id === id);
    return s ? { ...s } : undefined;
  }

  listSessions(): SessionNode[] {
    return this.sessionsDoc.sessions.map((s) => ({ ...s }));
  }

  async createFolder(name: string, parentId: string | null = null): Promise<string> {
    const id = `f-${Date.now().toString(36)}-${Math.floor(Math.random() * 1e6).toString(36)}`;
    this.sessionsDoc.folders.push({ id, name, children: [] });
    if (parentId) {
      const parent = this.sessionsDoc.folders.find((f) => f.id === parentId);
      if (parent) parent.children.push(id);
      else this.sessionsDoc.rootFolderIds.push(id);
    } else {
      this.sessionsDoc.rootFolderIds.push(id);
    }
    await this.persist('sessions');
    this.emit();
    return id;
  }

  /** 删除空文件夹（含会话/子文件夹时拒绝，避免误删整组会话） */
  async deleteFolder(id: string): Promise<void> {
    const idx = this.sessionsDoc.folders.findIndex((f) => f.id === id);
    if (idx < 0) return;
    const folder = this.sessionsDoc.folders[idx];
    if (folder.children.length > 0) {
      throw new Error(`文件夹「${folder.name}」非空，请先移出其中的会话与子文件夹`);
    }
    this.sessionsDoc.folders.splice(idx, 1);
    this.sessionsDoc.rootFolderIds = this.sessionsDoc.rootFolderIds.filter((r) => r !== id);
    for (const f of this.sessionsDoc.folders) {
      const ci = f.children.indexOf(id);
      if (ci >= 0) f.children.splice(ci, 1);
    }
    await this.persist('sessions');
    this.emit();
  }

  // ---------------- 会话 CRUD（SES-001） ----------------

  async saveSession(input: SaveSessionInput, id?: string): Promise<SessionNode> {
    const now = new Date().toISOString();
    const existing = id ? this.sessionsDoc.sessions.find((s) => s.id === id) : undefined;
    if (existing) {
      if (existing.locked) throw new LockedSessionError(existing.name);
      existing.kind = input.kind;
      existing.name = input.name;
      existing.specs = { ...input.specs };
      existing.overrides = input.overrides ? { ...input.overrides } : existing.overrides;
      if (input.state) existing.state = { ...input.state };
      if (input.folderId !== undefined) this.moveToFolder(existing, input.folderId);
      existing.updatedAt = now;
      await this.persist('sessions');
      this.emit();
      return { ...existing };
    }
    const node: SessionNode = {
      id: newSessionId(),
      kind: input.kind,
      name: input.name,
      locked: false,
      createdAt: now,
      updatedAt: now,
      specs: { ...input.specs },
      overrides: input.overrides ? { ...input.overrides } : {},
      state: input.state ? { ...input.state } : {},
      folderId: input.folderId ?? null,
    };
    this.sessionsDoc.sessions.push(node);
    if (node.folderId) {
      const folder = this.sessionsDoc.folders.find((f) => f.id === node.folderId);
      if (folder) folder.children.push(node.id);
      else node.folderId = null;
    }
    await this.persist('sessions');
    this.emit();
    return { ...node };
  }

  /** 另存为：以现有会话为模板创建新命名会话（默认名加"副本"） */
  async saveAsSession(sourceId: string, name?: string, folderId?: string | null): Promise<SessionNode> {
    const src = this.sessionsDoc.sessions.find((s) => s.id === sourceId);
    if (!src) throw new Error(`会话不存在: ${sourceId}`);
    return this.saveSession(
      {
        kind: src.kind,
        name: name ?? `${src.name} 副本`,
        specs: src.specs,
        overrides: src.overrides,
        folderId: folderId !== undefined ? folderId : src.folderId,
        state: src.state,
      },
      undefined
    );
  }

  async renameSession(id: string, name: string): Promise<void> {
    const s = this.sessionsDoc.sessions.find((x) => x.id === id);
    if (!s) throw new Error(`会话不存在: ${id}`);
    if (s.locked) throw new LockedSessionError(s.name);
    s.name = name;
    s.updatedAt = new Date().toISOString();
    await this.persist('sessions');
    this.emit();
  }

  async deleteSession(id: string): Promise<void> {
    const idx = this.sessionsDoc.sessions.findIndex((s) => s.id === id);
    if (idx < 0) return;
    const [node] = this.sessionsDoc.sessions.splice(idx, 1);
    for (const f of this.sessionsDoc.folders) {
      const ci = f.children.indexOf(id);
      if (ci >= 0) f.children.splice(ci, 1);
    }
    this.stateDoc.mru.sessions = this.stateDoc.mru.sessions.filter((x) => x !== id);
    void node;
    await this.persist('sessions');
    await this.persist('state');
    this.emit();
  }

  async setLocked(id: string, locked: boolean): Promise<void> {
    const s = this.sessionsDoc.sessions.find((x) => x.id === id);
    if (!s) throw new Error(`会话不存在: ${id}`);
    s.locked = locked;
    s.updatedAt = new Date().toISOString();
    await this.persist('sessions');
    this.emit();
  }

  /** 保存会话实例状态（展开/显示过滤器等）；锁定会话跳过（FR-SES-001） */
  async saveSessionState(id: string, patch: Partial<SessionState>): Promise<void> {
    const s = this.sessionsDoc.sessions.find((x) => x.id === id);
    if (!s) return;
    if (s.locked) return;
    s.state = { ...s.state, ...patch };
    s.updatedAt = new Date().toISOString();
    await this.persist('sessions');
    this.emit();
  }

  private moveToFolder(node: SessionNode, folderId: string | null): void {
    for (const f of this.sessionsDoc.folders) {
      const ci = f.children.indexOf(node.id);
      if (ci >= 0) f.children.splice(ci, 1);
    }
    node.folderId = folderId;
    if (folderId) {
      const folder = this.sessionsDoc.folders.find((f) => f.id === folderId);
      if (folder) folder.children.push(node.id);
      else node.folderId = null;
    }
  }

  // ---------------- MRU（state.json） ----------------

  getMruSessions(): string[] {
    return [...this.stateDoc.mru.sessions];
  }

  getMruPaths(kind: SessionKind): { left: string[]; right: string[] } {
    return {
      left: [...(this.stateDoc.mru.paths[kind]?.left ?? [])],
      right: [...(this.stateDoc.mru.paths[kind]?.right ?? [])],
    };
  }

  /** 打开会话/路径时登记：命名会话进 MRU sessions；路径按类型进 MRU paths */
  async recordOpened(kind: SessionKind, specs: SessionSpecs, sessionId?: string): Promise<void> {
    const max = this.stateDoc.mru.max;
    if (sessionId) {
      this.stateDoc.mru.sessions = pushMru(this.stateDoc.mru.sessions, sessionId, max);
    }
    const l = normalizePathInput(specs.left);
    const r = normalizePathInput(specs.right);
    const entry = this.stateDoc.mru.paths[kind] ?? { left: [], right: [] };
    entry.left = l ? pushMru(entry.left, l, max) : entry.left;
    entry.right = r ? pushMru(entry.right, r, max) : entry.right;
    this.stateDoc.mru.paths[kind] = entry;
    await this.persist('state');
    this.emit();
  }

  // ---------------- 设置作用域（SES-006） ----------------

  getKindDefaults(kind: SessionKind): SettingsTree {
    return this.kindDefaults.kinds[kind] ? { ...this.kindDefaults.kinds[kind] } : {};
  }

  getGlobalSettings(): SettingsTree {
    return { ...this.globalSettings.settings };
  }

  /** 生效设置 = global → kind → session（无父会话链时的 UI 读取口径） */
  resolveSessionEffective(kind: SessionKind, sessionOverrides?: SettingsTree) {
    return resolveEffective({
      global: this.globalSettings.settings,
      kind: this.kindDefaults.kinds[kind],
      session: sessionOverrides,
    });
  }

  /**
   * 作用域写回（FR-SES-006）：把 patch 写入所选层。
   * - session：合并进会话节点 overrides；
   * - kind / global：合并进对应文档；
   * - parent-*：由调用方（子会话设置对话框）以父会话 overrides 承载，此处返回 patch 让调用方处理。
   */
  async writeOverride(
    scope: SettingsScope,
    kind: SessionKind,
    sessionId: string | null,
    patch: SettingsTree
  ): Promise<void> {
    if (scope === 'parent-pair' || scope === 'parent-kind') {
      // 父会话层写回需要父会话上下文，由调用方直接写父会话 overrides
      throw new Error(`作用域 ${scope} 需要父会话上下文，请通过父会话写回`);
    }
    if (scope === 'session') {
      if (!sessionId) throw new Error('会话作用域需要会话 id');
      const s = this.sessionsDoc.sessions.find((x) => x.id === sessionId);
      if (!s) throw new Error(`会话不存在: ${sessionId}`);
      if (s.locked) throw new LockedSessionError(s.name);
      s.overrides = applyPatchToLayer(s.overrides, patch);
      s.updatedAt = new Date().toISOString();
      await this.persist('sessions');
    } else if (scope === 'kind') {
      this.kindDefaults.kinds[kind] = applyPatchToLayer(this.kindDefaults.kinds[kind], patch);
      await this.persist('kindDefaults');
    } else {
      this.globalSettings.settings = applyPatchToLayer(this.globalSettings.settings, patch);
      await this.persist('settings');
    }
    this.emit();
  }

  /** UI 状态（state.json ui 节点） */
  getUiState(): StateDoc['ui'] {
    return { ...this.stateDoc.ui };
  }

  async setUiState(patch: Partial<StateDoc['ui']>): Promise<void> {
    this.stateDoc.ui = { ...this.stateDoc.ui, ...patch };
    await this.persist('state');
    this.emit();
  }

  /** 测试/调试：当前会话文档快照 */
  debugSessionsDoc(): SessionsDoc {
    return this.sessionsDoc;
  }
}

/** 锁定会话的写拒绝（SES-001：锁定防止意外修改） */
export class LockedSessionError extends Error {
  constructor(name: string) {
    super(`会话「${name}」已锁定，修改被拒绝`);
    this.name = 'LockedSessionError';
  }
}

/** 应用级单例（AppShell/HomeView/命令层共用；init 由 main/AppShell 启动时调用） */
export const sessionService = new SessionService();

export function allKinds(): readonly SessionKind[] {
  return SESSION_KINDS;
}

export type { EffectiveLayer };
