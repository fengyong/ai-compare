import { useState } from 'react';
import { S } from '../../strings';
import { effectiveFormats, type FileFormatsState } from './formatsModel';
import {
  elementsOfFormat,
  effectiveUnimportantElements,
  toggleElement,
  type CompareRulesState,
} from './rulesModel';
import * as st from './pageStyles';

/**
 * 设置 > 比较规则 页（P10 / FR-TXT-010 设置侧）：
 * - 重要性开关六项（空白三类/大小写/行尾/编码；缺省与引擎一致）；
 * - "不重要"元素勾选：按所选文件格式列出其语法元素（抽象层引用格式层，
 *   SRS 2.4 两层协作）；勾选写入 formatBindings[formatId]；
 * - 作用域下拉由 SettingsDialog 统一渲染（本页 scope = session-scoped：
 *   全局默认 / 此类会话默认（session-defaults.json）/ 仅用于此会话）。
 *
 * 与引擎的接线：开关 + 元素集映射为引擎 RulesSpec（rulesModel.toRulesSpec），
 * text/open 携带后经 ai-core 重要性键语法折叠生效（骨架不变，仅重要性重判）。
 */

interface Props {
  value: CompareRulesState;
  formatsState: FileFormatsState;
  onChange(next: CompareRulesState): void;
}

export function CompareRulesPage({ value, formatsState, onChange }: Props) {
  const formats = effectiveFormats(formatsState);
  const [formatId, setFormatId] = useState('builtin-cpp');
  const elements = elementsOfFormat(formatsState, formatId);
  const checked = effectiveUnimportantElements(value, formatId);

  const flags: Array<{ key: keyof CompareRulesState; label: string; help: string }> = [
    { key: 'leadingWsUnimportant', label: S.settings.rules.leadingWs, help: S.settings.rules.leadingWsHelp },
    { key: 'embeddedWsUnimportant', label: S.settings.rules.embeddedWs, help: S.settings.rules.embeddedWsHelp },
    { key: 'trailingWsUnimportant', label: S.settings.rules.trailingWs, help: S.settings.rules.trailingWsHelp },
    { key: 'caseUnimportant', label: S.settings.rules.caseInsensitive, help: S.settings.rules.caseHelp },
    { key: 'eolStyleUnimportant', label: S.settings.rules.eolStyle, help: S.settings.rules.eolHelp },
    { key: 'encodingUnimportant', label: S.settings.rules.encoding, help: S.settings.rules.encodingHelp },
  ];

  return (
    <div className="aic-rules-page">
      <p className="dim" style={st.pageHint}>{S.settings.rules.hint}</p>
      <div className="aic-rules-flags" style={st.rulesFlags}>
        {flags.map((f) => (
          <label key={f.key} className="aic-rules-flag" style={st.rulesFlag} title={f.help}>
            <input
              type="checkbox"
              checked={value[f.key] === true}
              onChange={(e) => onChange({ ...value, [f.key]: e.target.checked })}
            />
            {f.label}
          </label>
        ))}
      </div>

      <h4 style={st.pageHint}>{S.settings.rules.elementsTitle}</h4>
      <div className="aic-rules-format" style={st.rulesFormatRow}>
        <label>{S.settings.rules.formatLabel}</label>
        <select value={formatId} onChange={(e) => setFormatId(e.target.value)}>
          {formats.map((f) => (
            <option key={f.id} value={f.id} disabled={!f.enabled}>
              {f.description || f.id}
              {f.enabled ? '' : `（${S.settings.rules.disabledSuffix}）`}
            </option>
          ))}
        </select>
      </div>
      {elements.length === 0 ? (
        <p className="dim" style={st.pageHint}>{S.settings.rules.noElements}</p>
      ) : (
        <div className="aic-rules-elements" style={st.rulesElements}>
          {elements.map((name) => (
            <label key={name} className="aic-rules-element" style={st.rulesFlag}>
              <input
                type="checkbox"
                checked={checked.includes(name)}
                onChange={() => onChange(toggleElement(value, formatId, name))}
              />
              {name}
            </label>
          ))}
        </div>
      )}
      <p className="dim" style={st.pageHint}>{S.settings.rules.bindingHint}</p>
    </div>
  );
}
