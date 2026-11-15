import { useEffect, useRef } from 'react';

export interface ContextMenuItem {
  label: string;
  onSelect(): void;
  disabled?: boolean;
  /** 分隔线占位项（label 为空时渲染分隔线） */
  separator?: boolean;
}

/**
 * 通用右键菜单（会话树/标签栏共用；FR-HOME-002 右键会话、FR-WIN-001 标签右键）。
 * 点击任意处/Escape 关闭；菜单溢出视口时自动上翻。
 */
export function ContextMenu({
  x,
  y,
  items,
  onClose,
}: {
  x: number;
  y: number;
  items: ContextMenuItem[];
  onClose(): void;
}) {
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const close = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) onClose();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    window.addEventListener('mousedown', close, true);
    window.addEventListener('keydown', onKey, true);
    return () => {
      window.removeEventListener('mousedown', close, true);
      window.removeEventListener('keydown', onKey, true);
    };
  }, [onClose]);

  const style: React.CSSProperties = { left: x, top: y };
  return (
    <div className="aic-ctxmenu" ref={ref} style={style} role="menu">
      {items.map((item, i) =>
        item.separator ? (
          <div className="aic-ctxmenu-sep" key={`sep-${i}`} />
        ) : (
          <button
            key={`${item.label}-${i}`}
            role="menuitem"
            disabled={item.disabled}
            onClick={() => {
              onClose();
              item.onSelect();
            }}
          >
            {item.label}
          </button>
        ),
      )}
    </div>
  );
}
