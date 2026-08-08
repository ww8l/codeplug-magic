import { useEffect, useRef } from "react";

/// Scroll a reorderable list while a row is being dragged over its top or
/// bottom edge. HTML5 drag-and-drop does not scroll anything on its own, so
/// without this a row can only be moved within the slice of the list that
/// happens to be on screen.
///
/// Pass whether a drag is in flight; attach the returned ref to the list. The
/// element it lands on does not have to be the scroller — the nearest
/// scrollable ancestor (or the element itself) is what gets scrolled.

/** Band at each edge of the scroller that pulls the list along. */
const EDGE_PX = 48;
/** Speeds in px/sec: a crawl entering the band, full tilt at the edge. */
const MIN_SPEED = 60;
const MAX_SPEED = 900;
/** A drag that leaves the window stops reporting; give up after this long. */
const IDLE_MS = 200;

export function useDragAutoScroll<T extends HTMLElement>(dragging: boolean) {
  const ref = useRef<T>(null);

  useEffect(() => {
    const el = ref.current;
    if (!dragging || !el) return;
    const box = scrollParent(el);
    if (!box) return;

    let velocity = 0; // px/sec, signed
    let movedAt = 0;
    let framedAt = 0;
    let frame = 0;

    // Scroll by elapsed time rather than per frame, so the list travels at the
    // same speed on a 60Hz and a 120Hz display.
    const tick = (now: number) => {
      // Dragging out of the window silently stops the dragover stream, so the
      // last velocity we saw would otherwise scroll on forever.
      if (velocity === 0 || now - movedAt > IDLE_MS) {
        frame = 0;
        return;
      }
      const elapsed = Math.min(now - framedAt, IDLE_MS) / 1000;
      framedAt = now;
      box.scrollTop += velocity * elapsed;
      frame = requestAnimationFrame(tick);
    };

    // Listening on the scroller rather than the rows keeps this working over
    // the sticky header and the empty space past the last row.
    const onDragOver = (e: DragEvent) => {
      const r = box.getBoundingClientRect();
      const fromTop = e.clientY - r.top;
      const fromBottom = r.bottom - e.clientY;
      velocity =
        fromTop < EDGE_PX
          ? -speed(fromTop)
          : fromBottom < EDGE_PX
            ? speed(fromBottom)
            : 0;
      movedAt = performance.now();
      if (velocity !== 0 && frame === 0) {
        framedAt = movedAt;
        frame = requestAnimationFrame(tick);
      }
    };

    box.addEventListener("dragover", onDragOver);
    return () => {
      box.removeEventListener("dragover", onDragOver);
      if (frame) cancelAnimationFrame(frame);
    };
  }, [dragging]);

  return ref;
}

/// Ramp up as the pointer nears the edge, and hold at full speed past it so
/// overshooting the list doesn't run away.
function speed(distance: number) {
  const depth = Math.min(1, Math.max(0, (EDGE_PX - distance) / EDGE_PX));
  return MIN_SPEED + (MAX_SPEED - MIN_SPEED) * depth;
}

/// Nearest ancestor (starting with the element itself) that both scrolls and
/// has something to scroll.
function scrollParent(el: HTMLElement): HTMLElement | null {
  for (let n: HTMLElement | null = el; n; n = n.parentElement) {
    const overflowY = getComputedStyle(n).overflowY;
    if (
      (overflowY === "auto" || overflowY === "scroll") &&
      n.scrollHeight > n.clientHeight
    ) {
      return n;
    }
  }
  return null;
}
