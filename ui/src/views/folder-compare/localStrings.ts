/**
 * P6b 批新增文案（组件内局部常量）——待并入 strings.ts（该文件当前归并行
 * P10 agent 独占，本批权限仅 folder-compare/**；合入时逐键迁移并删除本模块）。
 */

export const F = {
  // ---- 编辑菜单（FR-DIR-050/§3.1.6 编辑菜单） ----
  editMenu: '编辑',
  editSelectFiles: '选择所有文件',
  editSelectNewer: (side: '左' | '右' | '两侧') => `选择较新的（${side}）`,
  editSelectOlder: (side: '左' | '右' | '两侧') => `选择较旧的（${side}）`,
  editSelectOrphan: (side: '左' | '右' | '两侧') => `选择孤立的（${side}）`,
  editSelectedByCommand: (n: number) => `已选择 ${n} 项`,
  editNothingMatched: '无匹配项，选择未变化',

  // ---- 空格标记（BC 键盘惯例） ----
  markOnly: '仅显示标记',
  markOnlyHint: '只显示被空格键标记的行（会话内缓存，与排序/选择互不影响）',
  markedBadge: '已标记',
  markedCount: (n: number) => `标记 ${n} 项`,
  markHint: '空格键：标记/取消标记当前选中行',

  // ---- 批量重命名（FR-DIR-064） ----
  batchRename: '批量重命名…',
  batchRenameTitle: '批量重命名',
  batchRenameModeDos: 'DOS 掩码',
  batchRenameModeRegex: '正则表达式',
  batchRenameMaskLabel: '掩码（* 原名基底 / ? 保留原字符 / # 计数器）',
  batchRenameCounterLabel: '计数器起始',
  batchRenamePatternLabel: '旧掩码（正则）',
  batchRenameTemplateLabel: '新掩码模板（$1 组引用）',
  batchRenamePreviewTitle: '预览（旧名 → 新名）',
  batchRenameColOld: '旧名',
  batchRenameColNew: '新名',
  batchRenameColNote: '状态',
  batchRenameUnchanged: '不变',
  batchRenameKeepOriginal: '未命中，保留原名',
  batchRenameConflictCount: (n: number) => `冲突/非法 ${n} 项（已拦截执行）`,
  batchRenameExecCount: (n: number) => `将重命名 ${n} 项`,
  batchRenameNoSelection: '请先选择要重命名的行',
  batchRenameRegexBad: '正则不可用',
  batchRenameHintDos: '示例：IMG####.jpg（计数器）· *.bak（保留原名换扩展名）· abc?.bak（逐位保留原字符）',
  batchRenameHintRegex: '示例：查找 `.*(\\d\\.txt)` 替换 `xyz$1`；未命中的行保留原名',

  // ---- 快速比较（FR-DIR-013） ----
  quickCompare: '快速比较',
  quickCompareTitle: '快速比较',
  quickCompareDesc: '摘要对话框：仅展示两侧 size/mtime 与状态判定，不打开完整并排视图。',
  quickCompareCol: (label: string) => label,
  quickCompareSameSize: '大小相同',
  quickCompareDiffSize: '大小不同',
  quickCompareSameTime: '时间相同',
  quickCompareDiffTime: '时间不同',
  quickCompareQuickVerdict: '本次结果为「快速判定」：仅比较大小/时间戳等快速项，未读取文件内容（FR-DIR-010）。',
  quickCompareContentVerdict: '本次结果基于内容比较（已读取文件），非快速判定。',
  quickComparePairVerdict: '选中两项互相比较（FR-DIR-013：选中两个文件无论是否相对均成对比较；选中一个则与对面比较）。',
  quickCompareSingleVerdict: '单行两侧互相比较（与对面项目成对比较）。',
  quickCompareMissing: '（该侧不存在）',

  // ---- 复制文件名/路径（FR-DIR-066 复制文件名） ----
  ctxCopyName: '复制文件名',
  ctxCopyFullPath: '复制完整路径',
  ctxCopyRelPath: '复制相对路径',
  copyDone: (n: number) => `已复制 ${n} 项到剪贴板`,
  copyFailed: '复制失败（Tauri 环境剪贴板受限）；内容已打印到控制台，请手动复制',

  // ---- 基文件夹导航（FR-DIR-003 可编辑面包屑） ----
  navOpenHere: (side: string) => `以此层作为${side}侧基文件夹打开新会话`,
  navUp: (side: string) => `${side}侧上一级`,
  navEdit: (side: string) => `编辑${side}侧路径（回车打开新会话）`,
  navSwap: '交换左右基文件夹（新会话）',
  navInvalidPath: '路径为空，无法打开会话',
  navLeft: '左',
  navRight: '右',
} as const;
