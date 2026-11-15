import type { SessionKind } from '../session/model';
import { KIND_META, SESSION_KINDS } from '../session/model';
import type { SessionService } from '../session/service';

/**
 * 视图插件注册表（docs/03 §3.4.2 M31 视图插件体系）。
 *
 * 每类会话一个 ViewPlugin（id/能力声明/主页入口/命令集），主页大按钮与
 * 设置对话框按注册表驱动渲染；引擎能力协商后可过滤不可用项（P13 主题/i18n
 * 注入主题时仅替换 ViewCtx，不改变本注册表结构）。
 * 当前视图实现体仍为各 views/*（React 组件），注册表先统一"元数据 + 命令 +
 * 能力"面，createComponent 挂接在 P5/P6 各视图完备化时收敛进插件。
 */

export interface ViewCaps {
  /** 窗格数（文本/目录 2、合并 3、编辑 1） */
  panes: 1 | 2 | 3;
  editable: boolean;
  reports: boolean;
  /** 并排/上下布局切换（FR-UI-005） */
  layoutSwitch: boolean;
}

export type ViewId = SessionKind | 'home';

export interface ViewPlugin {
  id: ViewId;
  /** 主页大按钮排序（home 排最前不参与大按钮） */
  order: number;
  icon: string;
  title: string;
  /** 已实现视图；未实现项在主页置灰并注明阶段（FR-HOME-001） */
  implemented: boolean;
  /** 未实现时的预期阶段标注（tooltip） */
  phaseNote?: string;
  capabilities: ViewCaps;
  /** 本视图命令集（菜单/工具栏/快捷键统一来源，对齐 SRS 附录 F） */
  commands: string[];
}

export class ViewRegistry {
  private readonly plugins = new Map<ViewId, ViewPlugin>();

  register(plugin: ViewPlugin): void {
    if (this.plugins.has(plugin.id)) {
      throw new Error(`视图插件 id 重复: ${plugin.id}`);
    }
    this.plugins.set(plugin.id, plugin);
  }

  get(id: ViewId): ViewPlugin | undefined {
    return this.plugins.get(id);
  }

  /** 主页大按钮口径：按 order 升序的全部会话类型插件 */
  listByOrder(): ViewPlugin[] {
    return [...this.plugins.values()].sort((a, b) => a.order - b.order);
  }

  /** 已实现的会话类型（打开会话/新建分支入口） */
  implementedKinds(): SessionKind[] {
    return this.listByOrder()
      .filter((p) => p.implemented && p.id !== 'home')
      .map((p) => p.id as SessionKind);
  }
}

/** 内置六类视图插件注册表（会话类型能力声明来自 docs/03 §3.4.2 表） */
export function createDefaultRegistry(): ViewRegistry {
  const reg = new ViewRegistry();
  reg.register({
    id: 'home',
    order: 0,
    icon: '🏠',
    title: '主页',
    implemented: true,
    capabilities: { panes: 1, editable: false, reports: false, layoutSwitch: false },
    commands: ['app.settings', 'app.showSessionPanel'],
  });
  const defs: Array<{
    kind: SessionKind;
    order: number;
    caps: ViewCaps;
    commands: string[];
  }> = [
    {
      kind: 'folder-compare',
      order: 1,
      caps: { panes: 2, editable: false, reports: false, layoutSwitch: false },
      commands: [
        'session.save',
        'session.saveAs',
        'session.lock',
        'folder.copyToLeft',
        'folder.copyToRight',
        'folder.markSelection',
      ],
    },
    {
      kind: 'text-compare',
      order: 2,
      caps: { panes: 2, editable: false, reports: false, layoutSwitch: true },
      commands: ['session.save', 'session.saveAs', 'session.lock'],
    },
    // P8：文件夹同步/合并视图接入（SYNC-001~005 / DMRG-001~003）
    {
      kind: 'folder-sync',
      order: 3,
      caps: { panes: 2, editable: false, reports: true, layoutSwitch: false },
      commands: ['session.save', 'session.saveAs', 'session.lock', 'sync.run', 'sync.swapSides'],
    },
    {
      kind: 'folder-merge',
      order: 4,
      caps: { panes: 3, editable: false, reports: true, layoutSwitch: false },
      commands: [
        'session.save',
        'session.saveAs',
        'session.lock',
        'merge.decideLeft',
        'merge.decideRight',
        'merge.skip',
      ],
    },
    // 以下三类：视图本体属 P9，注册元数据占位（主页置灰 + 阶段 tooltip）
    {
      kind: 'table-compare',
      order: 5,
      caps: { panes: 2, editable: false, reports: false, layoutSwitch: true },
      commands: [],
    },
    {
      kind: 'hex-compare',
      order: 6,
      caps: { panes: 2, editable: false, reports: false, layoutSwitch: true },
      commands: [],
    },
    {
      kind: 'picture-compare',
      order: 7,
      caps: { panes: 2, editable: false, reports: false, layoutSwitch: true },
      commands: [],
    },
    // 队列 3：三向文本合并（FR-TMRG-001~007；消费 P7 的 text/merge3/* 四方法）
    {
      kind: 'text-merge',
      order: 8,
      // 三源窗格（左/右/中）+ 可编辑输出窗格；BC 的合并视图允许编辑输出
      caps: { panes: 3, editable: true, reports: false, layoutSwitch: false },
      commands: [
        'session.save',
        'session.saveAs',
        'session.lock',
        'merge3.takeLeft',
        'merge3.takeRight',
        'merge3.takeBase',
        'merge3.manual',
      ],
    },
  ];
  for (const d of defs) {
    const meta = KIND_META[d.kind];
    reg.register({
      id: d.kind,
      order: d.order,
      icon: meta.icon,
      title: meta.title,
      implemented: meta.implemented,
      phaseNote: meta.implemented ? undefined : `${meta.title}视图属 ${meta.phase} 阶段交付`,
      capabilities: d.caps,
      commands: d.commands,
    });
  }
  return reg;
}

/** 应用级单例 */
export const viewRegistry = createDefaultRegistry();

// ------------------------------------------------------------------
// ViewCtx（docs/03 §3.4.2：createView(ctx) 的注入面）
// ------------------------------------------------------------------

/** 剪贴板网关（FR-CLIP-004 子集：P4 至少供复制命令与剪贴板比较命令使用） */
export interface ClipboardGateway {
  writeText(text: string): Promise<void>;
  readText(): Promise<string>;
}

export interface ViewCtx {
  /** 会话服务（保存/另存/锁定/MRU/设置作用域） */
  sessions: SessionService;
  /** 剪贴板 */
  clipboard: ClipboardGateway;
  /** 状态栏一次性提示 */
  notice(msg: string): void;
}

function defaultClipboard(): ClipboardGateway {
  return {
    async writeText(text: string) {
      if (typeof navigator !== 'undefined' && navigator.clipboard) {
        await navigator.clipboard.writeText(text);
        return;
      }
      throw new Error('当前环境不支持剪贴板写入');
    },
    async readText() {
      if (typeof navigator !== 'undefined' && navigator.clipboard) {
        return navigator.clipboard.readText();
      }
      throw new Error('当前环境不支持剪贴板读取');
    },
  };
}

/** 状态栏提示出口（AppShell 挂载时注册；避免本模块与壳/store 循环依赖） */
let noticeSink: ((msg: string) => void) | null = null;

export function setNoticeSink(fn: ((msg: string) => void) | null): void {
  noticeSink = fn;
}

/** 构建视图上下文（AppShell 挂载时构建一次，随服务单例生命周期） */
export function createViewCtx(sessions: SessionService, clipboard = defaultClipboard()): ViewCtx {
  return {
    sessions,
    clipboard,
    notice(msg: string) {
      if (noticeSink) noticeSink(msg);
    },
  };
}

/** 全部已注册的会话类型 id（供新建分支/设置对话框遍历） */
export function allRegisteredKinds(): readonly SessionKind[] {
  return SESSION_KINDS;
}
