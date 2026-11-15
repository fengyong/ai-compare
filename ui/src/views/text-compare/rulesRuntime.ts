import { sessionService } from '../../session/service';
import {
  defaultCompareRulesState,
  toRulesSpec,
  type CompareRulesState,
} from '../../shell/settings/rulesModel';
import {
  defaultFileFormatsState,
  effectiveFormats,
  firstMatchingFormat,
  type FileFormatsState,
} from '../../shell/settings/formatsModel';
import type { RulesSpec } from '../../protocol/types';

// ---- P10 运行时接线：设置页「比较规则」→ text/open rules 参数 ----
// 数据源：kindDefaults('text-compare').compareRules（作用域=类型默认层）+
// 全局 settings 的 fileFormats（自定义覆盖参与掩码检测）。读取失败静默
// 回退引擎默认（undefined），与「未配置规则」语义一致。
export function effectiveRulesFor(leftPath: string, rightPath: string): RulesSpec | undefined {
  try {
    const tree = sessionService.getKindDefaults('text-compare');
    const group = (tree['compareRules'] ?? {}) as Partial<CompareRulesState>;
    if (Object.keys(group).length === 0) return undefined;
    const state: CompareRulesState = { ...defaultCompareRulesState(), ...group };
    const settingsTree = sessionService.getGlobalSettings();
    const formatsGroup = (settingsTree['fileFormats'] ?? {}) as Partial<FileFormatsState>;
    const formatsState: FileFormatsState = { ...defaultFileFormatsState(), ...formatsGroup };
    const formats = effectiveFormats(formatsState);
    const leftName = leftPath.split('/').pop() ?? leftPath;
    const rightName = rightPath.split('/').pop() ?? rightPath;
    const fmt = firstMatchingFormat(formats, leftName) ?? firstMatchingFormat(formats, rightName);
    return toRulesSpec(state, fmt?.id ?? null);
  } catch {
    return undefined; // 设置层损坏/未初始化时不阻断比较（引擎默认语义）
  }
}
