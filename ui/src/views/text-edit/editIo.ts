/**
 * 单窗格编辑器 ⇄ 引擎的写入口（A3）。
 *
 * 视图的"防抖同步/卸载冲刷"与保存流程的"锁内收敛"必须是**同一份**载荷语义：
 * 编辑器只编辑左侧（`side=left`），EOF 标记只在真变化时给出（`undefined` = 不改变，
 * 归一成协议要求的 `leftTrailingEol: null`）。两处各写一份必然漂移——漂移的代价
 * 是"末尾换行"在保存链路上被悄悄丢掉。
 */

import * as engine from '../../ipc/engineProcess';
import { textEditParams } from '../../protocol/editParams';
import type { TextEditOp } from '../../protocol/types';
import type { EditSyncIO } from './editModel';

/** 编辑器会话的 `text/edit` 适配器（左侧 + EOF 标记） */
export function editorSyncIo(sessionId: string): EditSyncIO {
  return {
    edit: (edits: TextEditOp[], trailing: boolean | undefined) =>
      engine.editText(
        textEditParams(
          sessionId,
          edits,
          trailing === undefined ? {} : { leftTrailingEol: trailing },
        ),
      ),
  };
}
