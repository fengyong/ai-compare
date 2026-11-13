/**
 * 界面文案集中管理（中文优先，为 i18n 预留）。
 * docs/03 §6.7：P13 阶段接入 i18next（zh-CN/en 全量语言包）；
 * 在此之前所有用户可见字符串从这里引用，键名保持稳定以便平滑迁移。
 */
export const S = {
  app: {
    title: 'ai-compare',
    homeTab: '主页',
    newTab: '+',
    closeTab: '关闭标签页',
  },

  engine: {
    connecting: '正在连接引擎…',
    ready: '引擎就绪',
    mock: '演示模式',
    degraded: '引擎不可用',
    retry: '重试连接',
    useMock: '改用演示数据',
    bannerTitle: '无法连接比较引擎',
    missingHint:
      '未找到引擎可执行文件 ai-engine。请先在仓库根执行 cargo build -p ai-engine 构建引擎（产物位于 target/debug/ai-engine）；或在浏览器中直接 pnpm dev 使用内置演示数据。',
    bannerInfo: '演示模式：当前使用内置模拟引擎，数据非真实文件系统。',
  },

  home: {
    title: '开始一次比较',
    subtitle: '选择左右两侧路径，创建文件夹比较会话（P0：快速测试判等 + 文本子会话）',
    leftPath: '左侧路径',
    rightPath: '右侧路径',
    leftPlaceholder: '/path/to/left',
    rightPlaceholder: '/path/to/right',
    start: '开始比较',
    sample: '填充示例路径',
    sampleLeft: '/demo/project-v1',
    sampleRight: '/demo/project-v2',
    pathRequired: '请输入左右两个路径',
  },

  folder: {
    nameCol: '名称',
    sizeCol: '大小',
    mtimeCol: '已修改',
    contentCol: '内容',
    leftTag: '左',
    rightTag: '右',
    scanning: '扫描中',
    scanDone: '扫描完成',
    scanCancelled: '已取消',
    scanError: '扫描出错',
    unknown: '未知',
    openFail: '打开失败',
    dirNotSupported: 'P0 暂不支持打开目录/压缩包行（目录展开属 P2/P6 范围）',
    orphanHint: '孤立项（另一侧不存在），无法打开子会话',
    stats: {
      same: '相同',
      contentDiff: '内容不同',
      quickDiff: '疑似不同',
      leftOnly: '仅左侧',
      rightOnly: '仅右侧',
      dirSame: '目录相同',
    },
    status: {
      same: '相同',
      contentDiff: '内容不同',
      quickDiff: '疑似不同（按时间判定，未验证内容）',
      leftOnly: '仅左侧',
      rightOnly: '仅右侧',
      dirSame: '目录（两侧均存在）',
      unknown: '未知',
    },
  },

  text: {
    paneLeft: '左侧',
    paneRight: '右侧',
    firstDiff: '第一处差异',
    prevDiff: '上一处差异',
    nextDiff: '下一处差异',
    sectionsLabel: '差异部分',
    equalSectionsLabel: '相同部分',
    loading: '正在加载比较结果…',
    loadFail: '文本比较加载失败',
    emptyFile: '（空文件）',
    unsupportedView: '该文件类型的视图尚未实现（hex/表格等视图属 P9 范围）',
    linesUnit: '行',
  },

  tab: {
    folderTitleSuffix: '文件夹比较',
    textTitleSuffix: '文本比较',
  },
} as const;
