import { useMemo, useState } from 'react';
import { sessionService } from '../../session/service';
import type { SessionKind, SettingsScope, SettingsTree } from '../../session/model';
import { SCOPE_LABELS, availableScopes, mergeTrees, resolveEffective } from '../../session/scopes';
import {
  SETTINGS_PAGES,
  pageById,
  readFieldValue,
  type FieldDef,
  type SettingsPage,
} from './settingsSchema';
import { S } from '../../strings';
import { useShell } from '../store';
import { FileFormatsPage } from './FileFormatsPage';
import { defaultFileFormatsState, type FileFormatsState } from './formatsModel';
import { CompareRulesPage } from './CompareRulesPage';
import { defaultCompareRulesState, type CompareRulesState } from './rulesModel';

/**
 * 设置对话框框架（FR-SES-002 / docs/04 §P4 交付 6）：
 * - 左侧分页导航（比较/同步/通用三页起步）+ schema 驱动表单（SchemaForm）；
 * - 底部作用域下拉（本会话/此类会话默认/全局默认；子会话两项 P6 开放）；
 * - 右侧"生效预览"面板：作用域切换/编辑后立即按四层合并预览（含继承来源标注）。
 *
 * 编辑为草稿模式：点「应用」经 SessionService.writeOverride 写回所选层并落盘。
 */

interface Props {
  initialPage?: string;
  /** 编辑对象：会话类型（默认取活动标签或 folder-compare） */
  kind?: SessionKind;
  /** 关联的会话标签（作用域"仅用于此会话"需要） */
  sessionTabId?: string;
}

interface DraftState {
  scope: SettingsScope;
  values: SettingsTree;
}

export function SettingsDialog({ initialPage, kind, sessionTabId }: Props) {
  const closeSettings = useShell((st) => st.closeSettings);
  const setNotice = useShell((st) => st.setNotice);
  const [page, setPage] = useState<SettingsPage>(pageById(initialPage));
  const [drafts, setDrafts] = useState<Record<string, DraftState>>({});
  const [scopes, setScopes] = useState<Record<string, SettingsScope>>({});

  const kindOfSession = useKind();
  const effectiveKind = kind ?? kindOfSession;
  const sessionScopeSupported = page.scope === 'session-scoped';
  const defaultScope: SettingsScope = sessionScopeSupported
    ? sessionTabId
      ? 'session'
      : 'kind'
    : 'global';
  const scope: SettingsScope = sessionScopeSupported ? (scopes[page.id] ?? defaultScope) : 'global';

  const loadLayer = (s: SettingsScope): SettingsTree => {
    if (s === 'global') return sessionService.getGlobalSettings();
    if (s === 'kind') return sessionService.getKindDefaults(effectiveKind);
    return sessionService.getSession(sessionTabId ?? '')?.overrides ?? {};
  };

  const draft: DraftState = drafts[`${page.id}:${scope}`] ?? {
    scope,
    values: cloneTree(loadLayer(scope)),
  };

  const setFieldValue = (group: string, field: FieldDef, value: unknown) => {
    setGroupKey(group, field.key, value);
  };

  /** 通用"组.键"写入（自定义结构化页以任意值形态写入，如数组/对象） */
  const setGroupKey = (group: string, key: string, value: unknown) => {
    setDrafts((prev) => {
      const key0 = `${page.id}:${scope}`;
      const base = prev[key0] ?? { scope, values: cloneTree(loadLayer(scope)) };
      return {
        ...prev,
        [key0]: {
          scope,
          values: { ...base.values, [group]: { ...base.values[group], [key]: value } },
        },
      };
    });
  };

  /** 整组多键写入（自定义结构化页的 onChange：一次提交多个键） */
  const setGroupValues = (group: string, values: Record<string, unknown>) => {
    setDrafts((prev) => {
      const key0 = `${page.id}:${scope}`;
      const base = prev[key0] ?? { scope, values: cloneTree(loadLayer(scope)) };
      return {
        ...prev,
        [key0]: {
          scope,
          values: { ...base.values, [group]: { ...base.values[group], ...values } },
        },
      };
    });
  };

  // P10 结构化页的受控值（缺省字段补齐 + 跨页引用）
  const formatsGroup = (draft.values['fileFormats'] ?? {}) as Partial<FileFormatsState>;
  const formatsDraft: FileFormatsState = { ...defaultFileFormatsState(), ...formatsGroup };
  const rulesGroup = (draft.values['compareRules'] ?? {}) as Partial<CompareRulesState>;
  const rulesDraft: CompareRulesState = { ...defaultCompareRulesState(), ...rulesGroup };
  const formatsStateForRules: FileFormatsState = (() => {
    const base = (sessionService.getGlobalSettings()['fileFormats'] ??
      {}) as Partial<FileFormatsState>;
    const d = drafts['fileFormats:global'];
    const overlay = (d?.values['fileFormats'] ?? {}) as Partial<FileFormatsState>;
    return { ...defaultFileFormatsState(), ...base, ...overlay };
  })();

  const setScope = (next: SettingsScope) => {
    setScopes((prev) => ({ ...prev, [page.id]: next }));
    setDrafts((prev) => {
      const key = `${page.id}:${next}`;
      if (prev[key]) return prev;
      return { ...prev, [key]: { scope: next, values: cloneTree(loadLayer(next)) } };
    });
  };

  /** 生效预览：当前草稿按所选层覆盖后四层合并 */
  const preview = useMemo(() => {
    const overlayFor = (s: SettingsScope, base: SettingsTree): SettingsTree | undefined => {
      const d = drafts[`${page.id}:${s}`];
      return d ? mergeTrees(base, d.values) : base;
    };
    return resolveEffective({
      global: overlayFor('global', sessionService.getGlobalSettings()),
      kind: effectiveKind
        ? overlayFor('kind', sessionService.getKindDefaults(effectiveKind))
        : undefined,
      session:
        sessionTabId && effectiveKind
          ? overlayFor('session', sessionService.getSession(sessionTabId)?.overrides ?? {})
          : undefined,
    });
  }, [drafts, page.id, effectiveKind, sessionTabId]);

  const apply = async () => {
    try {
      await sessionService.writeOverride(
        scope,
        effectiveKind,
        sessionTabId ? sessionTabId : null,
        draft.values,
      );
      setDrafts((prev) => {
        const next = { ...prev };
        delete next[`${page.id}:${scope}`];
        return next;
      });
      setNotice(`${S.settings.applied}（${SCOPE_LABELS[scope]}）`);
    } catch (err) {
      setNotice(`${S.settings.applyFail}: ${err instanceof Error ? err.message : String(err)}`);
    }
  };

  const group = page.id; // 值组 = 页 id（compare / sync / general）
  const scopeOptions = sessionScopeSupported
    ? availableScopes(false).filter((s) => s !== 'session' || !!sessionTabId)
    : (['global'] as const);

  return (
    <div className="aic-modal-mask" onClick={closeSettings}>
      <div
        className="aic-modal aic-settings"
        role="dialog"
        aria-label={S.settings.title}
        onClick={(e) => e.stopPropagation()}
      >
        <div className="aic-settings-head">
          <b>{S.settings.title}</b>
          <span className="dim">
            {S.settings.kindLabel}：{kindName(effectiveKind)}
          </span>
          <button className="close" aria-label={S.settings.close} onClick={closeSettings}>
            ×
          </button>
        </div>
        <div className="aic-settings-body">
          <nav className="aic-settings-nav">
            {SETTINGS_PAGES.map((p) => (
              <button
                key={p.id}
                className={p.id === page.id ? 'active' : ''}
                onClick={() => setPage(p)}
              >
                {p.icon} {p.title}
              </button>
            ))}
          </nav>
          <div className="aic-settings-form">
            {page.custom === 'fileFormats' ? (
              <FileFormatsPage
                value={formatsDraft}
                onChange={(next) =>
                  setGroupValues('fileFormats', {
                    custom: next.custom,
                    builtinOverrides: next.builtinOverrides,
                  })
                }
              />
            ) : page.custom === 'compareRules' ? (
              <CompareRulesPage
                value={rulesDraft}
                formatsState={formatsStateForRules}
                onChange={(next) =>
                  setGroupValues('compareRules', {
                    leadingWsUnimportant: next.leadingWsUnimportant,
                    embeddedWsUnimportant: next.embeddedWsUnimportant,
                    trailingWsUnimportant: next.trailingWsUnimportant,
                    caseUnimportant: next.caseUnimportant,
                    eolStyleUnimportant: next.eolStyleUnimportant,
                    encodingUnimportant: next.encodingUnimportant,
                    formatBindings: next.formatBindings,
                  })
                }
              />
            ) : (
              <SchemaForm
                page={page}
                layer={draft.values}
                group={group}
                onChange={(field, value) => setFieldValue(group, field, value)}
              />
            )}
            {sessionScopeSupported && (
              <div className="aic-settings-scope">
                <label>{S.settings.scopeLabel}</label>
                <select value={scope} onChange={(e) => setScope(e.target.value as SettingsScope)}>
                  {scopeOptions.map((s) => (
                    <option key={s} value={s}>
                      {SCOPE_LABELS[s]}
                    </option>
                  ))}
                </select>
                <span className="dim">{S.settings.scopeHint}</span>
              </div>
            )}
            <div className="aic-settings-actions">
              <button className="primary" onClick={() => void apply()}>
                {S.settings.apply}
              </button>
              <button onClick={closeSettings}>{S.settings.close}</button>
            </div>
          </div>
          <aside className="aic-settings-preview">
            <b>{S.settings.previewTitle}</b>
            <EffectivePreview preview={preview} />
          </aside>
        </div>
      </div>
    </div>
  );
}

function SchemaForm({
  page,
  layer,
  group,
  onChange,
}: {
  page: SettingsPage;
  layer: SettingsTree;
  group: string;
  onChange(field: FieldDef, value: unknown): void;
}) {
  return (
    <div className="aic-schema-form">
      {page.fields.map((f) => {
        const value = readFieldValue(layer, group, f);
        const disabled = !!f.phaseNote;
        return (
          <div className="aic-field" key={f.key} title={f.phaseNote ?? f.help ?? ''}>
            <label>
              {f.label}
              {disabled ? `（${f.phaseNote}）` : ''}
            </label>
            {f.type === 'boolean' && (
              <input
                type="checkbox"
                checked={value === true}
                disabled={disabled}
                onChange={(e) => onChange(f, e.target.checked)}
              />
            )}
            {f.type === 'number' && (
              <input
                type="number"
                value={Number(value)}
                min={f.min}
                max={f.max}
                step={f.step}
                disabled={disabled}
                onChange={(e) => onChange(f, Number(e.target.value))}
              />
            )}
            {f.type === 'select' && (
              <select
                value={String(value)}
                disabled={disabled}
                onChange={(e) => onChange(f, e.target.value)}
              >
                {f.options?.map((o) => (
                  <option key={o.value} value={o.value}>
                    {o.label}
                  </option>
                ))}
              </select>
            )}
            {f.type === 'text' && (
              <input
                type="text"
                value={String(value)}
                disabled={disabled}
                onChange={(e) => onChange(f, e.target.value)}
              />
            )}
          </div>
        );
      })}
    </div>
  );
}

function EffectivePreview({ preview }: { preview: ReturnType<typeof resolveEffective> }) {
  const groups = Object.keys(preview.values).sort();
  if (groups.length === 0) return <div className="dim">{S.settings.previewEmpty}</div>;
  const LAYER_LABEL: Record<string, string> = {
    global: '全局默认',
    kind: '类型默认',
    parentKind: '父会话（此类）',
    parentPair: '父会话（此对）',
    session: '本会话',
  };
  return (
    <div className="aic-preview-list">
      {groups.map((g) => (
        <div className="aic-preview-group" key={g}>
          <div className="aic-preview-group-head">
            {g}
            <span className="dim">{LAYER_LABEL[preview.origin[g]] ?? ''}</span>
          </div>
          <pre>{JSON.stringify(preview.values[g], null, 1)}</pre>
        </div>
      ))}
    </div>
  );
}

function cloneTree(t: SettingsTree): SettingsTree {
  const out: SettingsTree = {};
  for (const [g, v] of Object.entries(t)) out[g] = { ...v };
  return out;
}

function kindName(kind: SessionKind): string {
  const meta: Record<SessionKind, string> = {
    'folder-compare': '文件夹比较',
    'text-compare': '文本比较',
    'table-compare': '表格比较',
    'hex-compare': '十六进制比较',
    'picture-compare': '图片比较',
    'folder-sync': '文件夹同步',
    'folder-merge': '文件夹合并',
    'text-merge': '文本合并',
  };
  return meta[kind];
}

/** 编辑对象类型：优先活动标签关联会话的类型 */
function useKind(): SessionKind {
  const kind = useShell((st) => {
    const win = st.windows.find((w) => w.id === st.activeWindowId);
    const tab = win?.tabs.find((t) => t.id === win.activeTabId);
    return tab?.sessionLink?.kind ?? null;
  });
  return kind ?? 'folder-compare';
}
