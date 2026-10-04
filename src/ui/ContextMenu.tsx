// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
import { memo, useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { Check } from "lucide-react";

/** 一条菜单项。label 由调用处翻好译：菜单本身不认识任何业务。 */
export interface ContextMenuItem {
  key: string;
  label: string;
  icon?: ReactNode;
  /** 勾选态：工具、瓦片份数这种「当前是哪一档」的项，左边留一个勾。 */
  checked?: boolean;
  danger?: boolean;
  disabled?: boolean;
  onSelect: () => void;
}

interface MenuRequest {
  x: number;
  y: number;
  items: ContextMenuItem[];
}

const EVENT = "aipixel:context-menu";

/**
 * 在鼠标位置开菜单。调用处把 items 备齐，菜单只负责出现在哪儿、怎么收场。
 * preventDefault 要现在就做：等菜单渲染出来再拦，浏览器的原生菜单已经先弹了。
 */
export function openContextMenu(
  event: { clientX: number; clientY: number; preventDefault?: () => void },
  items: ContextMenuItem[],
) {
  if (items.length === 0) return;
  event.preventDefault?.();
  window.dispatchEvent(
    new CustomEvent<MenuRequest>(EVENT, { detail: { x: event.clientX, y: event.clientY, items } }),
  );
}

/** 菜单宿主：整个应用只挂一个，所有位置共用它。 */
function ContextMenuHost() {
  const [request, setRequest] = useState<MenuRequest | null>(null);
  const [shift, setShift] = useState({ x: 0, y: 0 });
  const boxRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const onOpen = (raw: Event) => {
      const detail = (raw as CustomEvent<MenuRequest>).detail;
      setShift({ x: 0, y: 0 });
      setRequest(detail);
    };
    window.addEventListener(EVENT, onOpen);
    return () => window.removeEventListener(EVENT, onOpen);
  }, []);

  // 贴边翻面：菜单不许探出窗口，靠近右/下边缘时往内平移，动画只走透明度。
  useLayoutEffect(() => {
    if (!request) return;
    const node = boxRef.current;
    if (!node) return;
    const box = node.getBoundingClientRect();
    const margin = 8;
    const flipX = Math.max(0, request.x + box.width + margin - window.innerWidth);
    const flipY = Math.max(0, request.y + box.height + margin - window.innerHeight);
    // 翻面之后还要夹回视口：菜单本身比可视区还高时（项特别多的右键菜单），
    // 只翻面会把上边距翻成负数，头几项就永远点不到了。
    const limitX = Math.max(margin, window.innerWidth - box.width - margin);
    const limitY = Math.max(margin, window.innerHeight - box.height - margin);
    const nextX = request.x - Math.min(Math.max(request.x - flipX, margin), limitX);
    const nextY = request.y - Math.min(Math.max(request.y - flipY, margin), limitY);
    if (nextX !== shift.x || nextY !== shift.y) setShift({ x: nextX, y: nextY });
  }, [request, shift]);

  useEffect(() => {
    if (!request) return;
    // 任何「点了别处」的信号都收菜单：pointerdown 而不是 click，
    // 按下那一刻就该收，别等一次完整的点击循环。
    const close = () => setRequest(null);
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        close();
      }
    };
    window.addEventListener("pointerdown", close);
    window.addEventListener("wheel", close, { passive: true });
    window.addEventListener("resize", close);
    window.addEventListener("blur", close);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("pointerdown", close);
      window.removeEventListener("wheel", close);
      window.removeEventListener("resize", close);
      window.removeEventListener("blur", close);
      window.removeEventListener("keydown", onKey);
    };
  }, [request]);

  if (!request) return null;
  return (
    <div
      ref={boxRef}
      className="ctx-menu"
      role="menu"
      style={{ left: request.x - shift.x, top: request.y - shift.y }}
      // 菜单自己的 pointerdown 不能顺手把它收掉：不然第一下点击就没了。
      onPointerDown={(event) => event.stopPropagation()}
    >
      {request.items.map((item) => (
        <button
          key={item.key}
          type="button"
          role="menuitem"
          className={`ctx-menu-item ${item.danger ? "danger" : ""}`}
          disabled={item.disabled}
          onClick={() => {
            setRequest(null);
            item.onSelect();
          }}
        >
          <span className="ctx-menu-icon">
            {item.checked ? <Check size={13} /> : item.icon}
          </span>
          <span className="ctx-menu-label">{item.label}</span>
        </button>
      ))}
    </div>
  );
}

/** 右键菜单的宿主平时什么都不渲染，memo 一层挡掉父组件本不必要的连锁渲染。 */
export default memo(ContextMenuHost);
