// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
import {
  useCallback,
  useEffect,
  useId,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
} from "react";

import {
  stripEdgesOf,
  stripScrollable,
  thumbBox,
  thumbDragScroll,
  type StripEdges,
  type StripMetrics,
} from "./strip";
import { hasPointerCapture, tryCapturePointer } from "../lib/pointer-capture";

/** 滑块的可抓下限：帧再多也留一寸捏得住，不然滑块细成头发反而更难用。 */
const MIN_THUMB = 26;
/** 方向键一次挪多少：跟着条子宽走，至少一步一帧多一点。 */
const KEY_STEP_RATIO = 0.25;

/** aria 用的最大滚动量，顺手挡成非负。 */
function maxScroll(metrics: StripMetrics): number {
  return Math.max(0, metrics.scrollWidth - metrics.clientWidth);
}

/**
 * 一条「一行放不下就往右滚」的横条。
 *
 * 帧条、坞里的帧/图层挑选条都是这个形态：抽帧补帧动辄几十帧，侧栏只有两三百
 * 像素，横着放、多出来的往右滚，绝不能换行——换行会把下面的小节顶出视野，
 * 帧条自己也看不全。
 *
 * 滚动条必须自己画：macOS 的滚动条悬浮且静止时整根隐形，Chrome/WebKit 现在
 * 连 ::-webkit-scrollbar 的尺寸声明都直接忽略（实测既不占布局也不绘制），
 * 标准 scrollbar-width 在 macOS 上照样隐形。结果就是帧被裁掉一半，用户既看不
 * 到滑块、也想不到能滚，只会得出「横向滚动根本没做」的结论。所以这里常驻画
 * 一根：能拖、能点轨道翻页、能键盘操作，两端再给渐隐提示还有内容。
 */
export default function HStrip({
  scrollerRef,
  className = "",
  label,
  onEdges,
  children,
}: {
  /** 调用方要自己读 scrollLeft 时把 ref 递进来（帧条靠它把选中帧滚进视野）。 */
  // 形状而不是 RefObject：React 18 的 RefObject.current 是只读的，而这里
  // 必须把内 ref 和调用方的 ref 同时指着同一个节点。
  scrollerRef?: { current: HTMLDivElement | null };
  className?: string;
  /** 无障碍名字：滑块读出来的就是这一条是什么。 */
  label: string;
  /** 两端状态回吐：父级拿它点亮翻页按钮。 */
  onEdges?: (edges: StripEdges) => void;
  children: ReactNode;
}) {
  const ownRef = useRef<HTMLDivElement | null>(null);
  const barRef = useRef<HTMLDivElement | null>(null);
  const thumbRef = useRef<HTMLDivElement | null>(null);
  const scrollerId = useId();
  const [edges, setEdges] = useState<StripEdges>({ canLeft: false, canRight: false });
  const [scrollable, setScrollable] = useState(false);

  // 内 ref 和外 ref 指着同一个滚动元素。滚动只能归一根管，不能一份在内一份在外。
  const attachScroller = useCallback(
    (node: HTMLDivElement | null) => {
      ownRef.current = node;
      if (scrollerRef) scrollerRef.current = node;
    },
    [scrollerRef],
  );

  const readMetrics = useCallback((): StripMetrics | null => {
    const node = ownRef.current;
    if (!node) return null;
    return {
      scrollWidth: node.scrollWidth,
      clientWidth: node.clientWidth,
      scrollLeft: node.scrollLeft,
    };
  }, []);

  // 滑块用 imperative 更新：横滚一帧一次 setState 是白烧渲染，而滑块位置只是
  // 毫秒级的视觉反馈，没资格进 React 调度。只有两端状态进 state——它一天翻不了
  // 几次，还要拿出去点亮父级的翻页按钮。
  const paint = useCallback(() => {
    const metrics = readMetrics();
    if (!metrics) return;
    const nextScrollable = stripScrollable(metrics);
    setScrollable((prev) => (prev === nextScrollable ? prev : nextScrollable));
    setEdges((prev) => {
      const next = stripEdgesOf(metrics);
      return prev.canLeft === next.canLeft && prev.canRight === next.canRight ? prev : next;
    });
    const thumb = thumbRef.current;
    if (!thumb || !nextScrollable) return;
    const track = barRef.current?.clientWidth ?? metrics.clientWidth;
    const box = thumbBox(metrics, track, MIN_THUMB);
    thumb.style.width = `${box.width}px`;
    thumb.style.transform = `translateX(${box.left}px)`;
    thumb.setAttribute("aria-valuemin", "0");
    thumb.setAttribute("aria-valuemax", String(Math.round(maxScroll(metrics))));
    thumb.setAttribute("aria-valuenow", String(Math.round(metrics.scrollLeft)));
  }, [readMetrics]);

  useEffect(() => {
    const node = ownRef.current;
    if (!node) return;
    let frame = 0;
    const schedule = () => {
      if (frame) return;
      frame = requestAnimationFrame(() => {
        frame = 0;
        paint();
      });
    };
    node.addEventListener("scroll", schedule, { passive: true });
    // 条子变窄（窗口拖动）、内容变宽（帧加减、换文档、改名）都要重算。
    const size = new ResizeObserver(schedule);
    size.observe(node);
    // 只观察条子本身不够：子格子自己变宽（改时长文案、改名）不触发 childList。
    // MutationObserver 回调里顺手把新格子也挂上尺寸监听。
    const kids = new MutationObserver(() => {
      watchKids();
      schedule();
    });
    kids.observe(node, { childList: true, subtree: true });
    const watchKids = () => {
      for (const child of Array.from(node.children)) {
        if (child instanceof HTMLElement) size.observe(child);
      }
    };
    watchKids();
    schedule();
    return () => {
      if (frame) cancelAnimationFrame(frame);
      node.removeEventListener("scroll", schedule);
      size.disconnect();
      kids.disconnect();
    };
  }, [paint]);

  // 两端状态真的变了才喊父级：按钮亮灭靠这一句。父级传内联闭包时这条 effect
  // 会跟着重跑，但那侧会拿新旧值比对后原样返回，多喊一次不花一次渲染。
  useEffect(() => {
    onEdges?.(edges);
  }, [edges, onEdges]);

  /**
   * 竖滚轮翻译成横滚。横条天生是横着的：触控板双指横滑走 deltaX 天生能滚，
   * 普通鼠标只有竖滚轮，不翻译的话滚一下条子纹丝不动，用户照样以为没做。
   *
   * 用 addEventListener 而非 React 的 onWheel：后者以 passive 注册，没法
   * preventDefault，滚到底还会把外层 panel-body 一起带着竖滚。
   */
  const onWheel = useCallback((event: WheelEvent) => {
    const node = ownRef.current;
    if (!node) return;
    // 双指缩放是页面级手势，不抢。
    if (event.ctrlKey) return;
    // 横滑优先：幅度大的那个方向才是用户想去的方向。
    const delta = Math.abs(event.deltaX) > Math.abs(event.deltaY) ? event.deltaX : event.deltaY;
    if (!delta) return;
    const before = node.scrollLeft;
    node.scrollLeft += delta;
    // 已经滚到尽头就不再吞事件，让外层面板接着竖滚。
    if (node.scrollLeft !== before) event.preventDefault();
  }, []);

  useEffect(() => {
    const node = ownRef.current;
    if (!node) return;
    node.addEventListener("wheel", onWheel, { passive: false });
    return () => node.removeEventListener("wheel", onWheel);
  }, [onWheel]);

  // 拖滑块：起点状态钉住，之后每次只按「总位移」换算，不按增量累加——拿当前
  // scrollLeft 当基数会把位移一层层叠上去，滑块越拖越飘，且永远追不上指针。
  const onThumbPointerDown = (event: ReactPointerEvent<HTMLDivElement>) => {
    const node = ownRef.current;
    const bar = barRef.current;
    const thumb = thumbRef.current;
    if (!node || !bar || !thumb) return;
    event.preventDefault();
    event.stopPropagation();
    const metrics = readMetrics();
    if (!metrics) return;
    const travel = Math.max(0, bar.clientWidth - thumb.offsetWidth);
    const startX = event.clientX;
    const move = (move: PointerEvent) => {
      node.scrollLeft = thumbDragScroll(metrics, travel, move.clientX - startX);
    };
    const done = (end: PointerEvent) => {
      if (hasPointerCapture(thumb, end.pointerId)) thumb.releasePointerCapture(end.pointerId);
      thumb.removeEventListener("pointermove", move);
      thumb.removeEventListener("pointerup", done);
      thumb.removeEventListener("pointercancel", done);
    };
    thumb.addEventListener("pointermove", move);
    thumb.addEventListener("pointerup", done);
    thumb.addEventListener("pointercancel", done);
    // 捕获放在监听之后：它是「划出滑块还跟手」的增强，不是启动拖动的前提。
    // 见 lib/pointer-capture.ts——裸调在前会抛 NotFoundError，把拖动整个废掉。
    tryCapturePointer(thumb, event.pointerId);
  };

  /** 点轨道空白处当翻页键：点在滑块左边往左一屏，右边往右一屏。 */
  const onBarPointerDown = (event: ReactPointerEvent<HTMLDivElement>) => {
    // 点在滑块上归滑块自己拖；只有落在轨道上才算翻页。
    if (event.target !== barRef.current) return;
    const node = ownRef.current;
    const bar = barRef.current;
    if (!node || !bar) return;
    const metrics = readMetrics();
    if (!metrics) return;
    const box = thumbBox(metrics, bar.clientWidth, MIN_THUMB);
    const x = event.clientX - bar.getBoundingClientRect().left;
    const dir = x < box.left + box.width / 2 ? -1 : 1;
    node.scrollBy({ left: dir * node.clientWidth, behavior: "smooth" });
  };

  /** 键盘也能滚：滑块可聚焦，方向键、翻页键、Home/End 都按滚动条的规矩来。 */
  const onThumbKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    const node = ownRef.current;
    if (!node) return;
    const step = Math.max(24, node.clientWidth * KEY_STEP_RATIO);
    switch (event.key) {
      case "ArrowLeft":
        node.scrollBy({ left: -step });
        break;
      case "ArrowRight":
        node.scrollBy({ left: step });
        break;
      case "PageUp":
        node.scrollBy({ left: -node.clientWidth });
        break;
      case "PageDown":
      case " ":
        node.scrollBy({ left: node.clientWidth });
        break;
      case "Home":
        node.scrollTo({ left: 0 });
        break;
      case "End":
        node.scrollTo({ left: node.scrollWidth });
        break;
      default:
        return;
    }
    event.preventDefault();
  };

  return (
    <div className="hstrip">
      <div ref={attachScroller} id={scrollerId} className={`hstrip-scroll ${className}`.trim()}>
        {children}
      </div>
      {/*
        滚动条只在真溢得出的时候渲染：帧不多时它占的那 13px 是纯浪费。
        滑块先按 CSS 里的默认尺寸站好，几何下一帧由 paint 校正，不会闪。
      */}
      {scrollable ? (
        <div className="hstrip-bar" ref={barRef} onPointerDown={onBarPointerDown}>
          <div
            className="hstrip-thumb"
            ref={thumbRef}
            role="scrollbar"
            tabIndex={0}
            aria-orientation="horizontal"
            aria-controls={scrollerId}
            aria-label={label}
            onPointerDown={onThumbPointerDown}
            onKeyDown={onThumbKeyDown}
          />
        </div>
      ) : null}
      {/* 两端渐隐：边缘像「被裁断了」还是「还有内容」，全看这一道。 */}
      {edges.canLeft ? <div className="hstrip-fade hstrip-fade-l" /> : null}
      {edges.canRight ? <div className="hstrip-fade hstrip-fade-r" /> : null}
    </div>
  );
}
