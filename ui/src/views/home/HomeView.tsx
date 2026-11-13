import { useState } from 'react';
import { useShell } from '../../shell/store';
import { S } from '../../strings';

/**
 * 主页（FR-HOME-001 的极简占位版）：两个路径选择器 + 「开始比较」。
 * 类型大按钮矩阵 / 会话树 / 最近会话 / 拖放启动属 P4 阶段交付。
 */
export function HomeView() {
  const openFolderTab = useShell((st) => st.openFolderTab);
  const [left, setLeft] = useState('');
  const [right, setRight] = useState('');

  const start = () => {
    if (!left.trim() || !right.trim()) {
      useShell.getState().setNotice(S.home.pathRequired);
      return;
    }
    openFolderTab(left.trim(), right.trim());
    setLeft('');
    setRight('');
  };

  return (
    <div className="aic-home">
      <div className="aic-home-card">
        <h1>{S.home.title}</h1>
        <div className="sub">{S.home.subtitle}</div>

        <div className="aic-home-row">
          <label style={{ width: 72, alignSelf: 'center' }}>{S.home.leftPath}</label>
          <input
            value={left}
            placeholder={S.home.leftPlaceholder}
            onChange={(e) => setLeft(e.target.value)}
            onKeyDown={(e) => e.key === 'Enter' && start()}
          />
        </div>
        <div className="aic-home-row">
          <label style={{ width: 72, alignSelf: 'center' }}>{S.home.rightPath}</label>
          <input
            value={right}
            placeholder={S.home.rightPlaceholder}
            onChange={(e) => setRight(e.target.value)}
            onKeyDown={(e) => e.key === 'Enter' && start()}
          />
        </div>

        <div className="aic-home-actions">
          <button
            title={S.home.sample}
            onClick={() => {
              setLeft(S.home.sampleLeft);
              setRight(S.home.sampleRight);
            }}
          >
            {S.home.sample}
          </button>
          <button className="primary" onClick={start}>
            {S.home.start}
          </button>
        </div>
      </div>
    </div>
  );
}
