import { useState } from 'react';
import type { FileFormatSpec, FormatViewKind, GrammarItemSpec } from '../../protocol/types';
import { S } from '../../strings';
import {
  effectiveFormats,
  exportFormatPack,
  importFormatPack,
  maskListText,
  newCustomFormat,
  parseMaskList,
  removeCustom,
  setBuiltinOverride,
  shadowedFormatIds,
  updateCustomElement,
  upsertCustom,
  validateFormatSpec,
  type FileFormatsState,
} from './formatsModel';
import * as st from './pageStyles';

/**
 * 设置 > 文件格式 页（P10 / FR-FMT-001 设置侧）：
 * - 有序列表（顺序即匹配优先级；首匹配生效）+ 启用勾选 + 掩码编辑；
 * - 被更高项遮蔽的格式红显（FR-FMT-001 遮蔽语义）；禁用项置灰；
 * - 自定义格式：语法元素编辑（元素名 + 语法项类别下拉 + 匹配串）；
 * - 导入/导出：BC 风格格式集（.bcpack / JSON 载体）。
 *
 * 组件只做展示与状态回写（value/onChange 受控），持久化经 SettingsDialog
 * 的草稿机制落到 settings.json 全局层（格式为程序级资源，FR-FMT-001）。
 */

interface Props {
  value: FileFormatsState;
  onChange(next: FileFormatsState): void;
}

const VIEW_KINDS: FormatViewKind[] = ['text', 'table', 'hex', 'picture', 'external'];

export function FileFormatsPage({ value, onChange }: Props) {
  const formats = effectiveFormats(value);
  const shadowed = shadowedFormatIds(formats);
  const [packText, setPackText] = useState('');
  const [message, setMessage] = useState('');

  const doImport = () => {
    if (!packText.trim()) {
      setMessage(S.settings.formats.importEmpty);
      return;
    }
    const r = importFormatPack(value, packText);
    if (r.ok) {
      onChange(r.state);
      setMessage(`${S.settings.formats.importOk} (${r.loaded})`);
    } else {
      setMessage(`${S.settings.formats.importFail}: ${r.error}`);
    }
  };

  const doExport = () => {
    setPackText(exportFormatPack(value));
    setMessage(S.settings.formats.exportDone);
  };

  const addFormat = () => {
    const id = `custom-${Date.now().toString(36)}`;
    onChange(upsertCustom(value, newCustomFormat(id)));
  };

  const saveCustom = (fmt: FileFormatSpec) => {
    const errors = validateFormatSpec(fmt);
    if (errors.length > 0) {
      setMessage(errors.join('；'));
      return;
    }
    onChange(upsertCustom(value, fmt));
    setMessage('');
  };

  return (
    <div className="aic-formats-page">
      <p className="dim" style={st.pageHint}>
        {S.settings.formats.hint}
      </p>
      <table className="aic-formats-table" style={st.formatsTable}>
        <thead>
          <tr>
            <th style={st.formatsTh}>{S.settings.formats.colOrder}</th>
            <th style={st.formatsTh}>{S.settings.formats.colEnabled}</th>
            <th style={st.formatsTh}>{S.settings.formats.colId}</th>
            <th style={st.formatsTh}>{S.settings.formats.colMasks}</th>
            <th style={st.formatsTh}>{S.settings.formats.colView}</th>
            <th style={st.formatsTh}>{S.settings.formats.colElements}</th>
            <th />
          </tr>
        </thead>
        <tbody>
          {formats.map((f, i) => (
            <FormatRow
              key={`${f.id}:${f.enabled ? 'on' : 'off'}:${maskListText(f.masks)}`}
              format={f}
              order={i + 1}
              shadowed={shadowed.has(f.id)}
              state={value}
              onChange={onChange}
              onMessage={setMessage}
              onSaveCustom={saveCustom}
            />
          ))}
        </tbody>
      </table>
      <div className="aic-formats-actions" style={st.rowActions}>
        <button onClick={addFormat}>{S.settings.formats.add}</button>
      </div>

      <h4 style={st.pageHint}>{S.settings.formats.packTitle}</h4>
      <div className="aic-formats-pack" style={st.packArea}>
        <textarea
          rows={6}
          style={st.packTextarea}
          value={packText}
          placeholder={S.settings.formats.packPlaceholder}
          onChange={(e) => setPackText(e.target.value)}
        />
        <div style={st.packButtons}>
          <button onClick={doImport}>{S.settings.formats.import}</button>
          <button onClick={doExport}>{S.settings.formats.export}</button>
        </div>
        {message && (
          <div className="aic-formats-message" style={st.messageLine}>
            {message}
          </div>
        )}
      </div>
    </div>
  );
}

function FormatRow({
  format,
  order,
  shadowed,
  state,
  onChange,
  onMessage,
  onSaveCustom,
}: {
  format: FileFormatSpec;
  order: number;
  shadowed: boolean;
  state: FileFormatsState;
  onChange(next: FileFormatsState): void;
  onMessage(msg: string): void;
  onSaveCustom(fmt: FileFormatSpec): void;
}) {
  const isBuiltin = format.id.startsWith('builtin-');
  const [masksText, setMasksText] = useState(maskListText(format.masks));
  const [editing, setEditing] = useState(false);

  const commitMasks = () => {
    const masks = parseMaskList(masksText);
    setMasksText(maskListText(masks));
    if (isBuiltin) {
      onChange(setBuiltinOverride(state, format.id, { masks }));
    } else {
      onSaveCustom({ ...format, masks });
    }
  };

  const toggleEnabled = () => {
    if (isBuiltin) {
      onChange(setBuiltinOverride(state, format.id, { enabled: !format.enabled }));
    } else {
      onSaveCustom({ ...format, enabled: !format.enabled });
    }
  };

  return (
    <>
      <tr
        className={format.enabled ? '' : 'disabled'}
        style={format.enabled ? undefined : st.disabledRow}
      >
        <td style={st.formatsTd}>{order}</td>
        <td style={st.formatsTd}>
          <input type="checkbox" checked={format.enabled} onChange={toggleEnabled} />
        </td>
        <td style={st.formatsTd}>
          {format.id}
          {shadowed && (
            <span className="aic-shadowed" style={st.shadowedMark}>
              （{S.settings.formats.shadowed}）
            </span>
          )}
          {!isBuiltin && (
            <button
              className="aic-formats-edit"
              style={st.editBtn}
              aria-label={`${S.settings.formats.edit} ${format.id}`}
              onClick={() => setEditing(!editing)}
            >
              {S.settings.formats.edit}
            </button>
          )}
        </td>
        <td>
          <input
            type="text"
            value={masksText}
            disabled={!format.enabled}
            onChange={(e) => setMasksText(e.target.value)}
            onBlur={commitMasks}
          />
        </td>
        <td style={st.formatsTd}>
          {isBuiltin ? (
            viewKindLabel(format.viewKind)
          ) : (
            <select
              value={format.viewKind}
              onChange={(e) =>
                onSaveCustom({ ...format, viewKind: e.target.value as FormatViewKind })
              }
            >
              {VIEW_KINDS.map((k) => (
                <option key={k} value={k}>
                  {viewKindLabel(k)}
                </option>
              ))}
            </select>
          )}
        </td>
        <td className="aic-formats-elements" style={st.elementsCell}>
          {format.grammar.map((el) => el.name).join('、') || '—'}
        </td>
        <td style={st.formatsTd}>
          {!isBuiltin && (
            <button
              aria-label={`${S.settings.formats.remove} ${format.id}`}
              onClick={() => {
                onChange(removeCustom(state, format.id));
                onMessage('');
              }}
            >
              {S.settings.formats.remove}
            </button>
          )}
        </td>
      </tr>
      {!isBuiltin && editing && (
        <tr className="aic-formats-editor-row">
          <td colSpan={7} style={st.editorCell}>
            <CustomGrammarEditor format={format} onSave={onSaveCustom} />
          </td>
        </tr>
      )}
    </>
  );
}

/** 自定义格式的语法元素最小编排（元素名 + 语法项类别下拉 + 匹配串） */
function CustomGrammarEditor({
  format,
  onSave,
}: {
  format: FileFormatSpec;
  onSave(fmt: FileFormatSpec): void;
}) {
  const stateHolder = { custom: [format], builtinOverrides: {} } as FileFormatsState;
  return (
    <div
      className="aic-grammar-editor"
      style={{ display: 'flex', flexDirection: 'column', gap: 6, padding: '4px 0' }}
    >
      {format.grammar.map((el, i) => {
        const item = el.items[0];
        const kind = item?.kind ?? 'delimited';
        return (
          <div className="aic-grammar-row" key={i} style={st.grammarRow}>
            <input
              type="text"
              value={el.name}
              aria-label={`${S.settings.formats.elementName}-${i}`}
              onChange={(e) => {
                const next = updateCustomElement(stateHolder, format.id, i, {
                  name: e.target.value,
                });
                onSave({ ...format, grammar: next.custom[0].grammar });
              }}
            />
            <select
              value={kind}
              aria-label={`${S.settings.formats.elementKind}-${i}`}
              onChange={(e) => {
                const next = updateCustomElement(stateHolder, format.id, i, {
                  kind: e.target.value as GrammarItemSpec['kind'],
                });
                onSave({ ...format, grammar: next.custom[0].grammar });
              }}
            >
              <option value="text">{S.settings.formats.kindText}</option>
              <option value="delimited">{S.settings.formats.kindDelimited}</option>
              <option value="list">{S.settings.formats.kindList}</option>
              <option value="column">{S.settings.formats.kindColumn}</option>
              <option value="rowTitle">{S.settings.formats.kindRowTitle}</option>
            </select>
            <input
              type="text"
              value={item?.pattern ?? item?.start ?? ''}
              aria-label={`${S.settings.formats.elementPattern}-${i}`}
              placeholder={S.settings.formats.elementPattern}
              onChange={(e) => {
                const next = updateCustomElement(stateHolder, format.id, i, {
                  pattern: e.target.value,
                });
                onSave({ ...format, grammar: next.custom[0].grammar });
              }}
            />
          </div>
        );
      })}
      <div className="dim" style={st.pageHint}>
        {S.settings.formats.grammarHint}
      </div>
    </div>
  );
}

function viewKindLabel(k: FormatViewKind): string {
  const labels: Record<FormatViewKind, string> = {
    text: S.settings.formats.viewText,
    table: S.settings.formats.viewTable,
    hex: S.settings.formats.viewHex,
    picture: S.settings.formats.viewPicture,
    external: S.settings.formats.viewExternal,
  };
  return labels[k];
}
