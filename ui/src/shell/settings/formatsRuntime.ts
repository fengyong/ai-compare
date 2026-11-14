import { sessionService } from '../../session/service';
import type { FileFormatSpec } from '../../protocol/types';
import { defaultFileFormatsState, effectiveFormats, type FileFormatsState } from './formatsModel';

/**
 * 运行时生效格式列表（FR-FMT-001 / FR-SEL-001）。
 *
 * formatsModel 是纯函数层，读设置属 IO，故单列此处：数据源与设置页「文件格式」
 * 同一份全局 settings `fileFormats` 组（内置覆盖 + 自定义追加）。设置层未初始化
 * 或损坏时回落内置表，不阻断打开文件。
 */
export function effectiveFormatsFromSettings(): FileFormatSpec[] {
  try {
    const group = (sessionService.getGlobalSettings()['fileFormats'] ??
      {}) as Partial<FileFormatsState>;
    return effectiveFormats({ ...defaultFileFormatsState(), ...group });
  } catch {
    return effectiveFormats(defaultFileFormatsState());
  }
}
