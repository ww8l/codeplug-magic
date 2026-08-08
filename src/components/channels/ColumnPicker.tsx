import { useEffect, useRef, useState } from "react";
import { Columns3, RotateCcw, GripVertical, X, Plus } from "lucide-react";
import clsx from "clsx";
import type { Channel } from "../../lib/types";
import { useDragAutoScroll } from "../../lib/dragAutoScroll";
import { Button } from "../ui";

interface ColumnOption {
  key: keyof Channel;
  label: string;
}

export function ColumnPicker({
  columns,
  visibleKeys,
  onChange,
  onReset,
}: {
  columns: ColumnOption[];
  visibleKeys: (keyof Channel)[];
  onChange: (keys: (keyof Channel)[]) => void;
  onReset: () => void;
}) {
  const [open, setOpen] = useState(false);
  const [dragIndex, setDragIndex] = useState<number | null>(null);
  const ref = useRef<HTMLDivElement>(null);
  const shownRef = useDragAutoScroll<HTMLDivElement>(dragIndex !== null);

  // Close on outside click or Escape.
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  const labelOf = (key: keyof Channel) =>
    columns.find((c) => c.key === key)?.label ?? String(key);

  // Shown columns in their current order; hidden ones in catalog order.
  const shown = visibleKeys;
  const hidden = columns.filter((c) => !visibleKeys.includes(c.key));

  const hide = (key: keyof Channel) => {
    if (visibleKeys.length === 1) return; // keep at least one
    onChange(visibleKeys.filter((k) => k !== key));
  };
  const show = (key: keyof Channel) => onChange([...visibleKeys, key]);

  // Live drag-reorder within the shown list.
  const onDragOver = (i: number) => (e: React.DragEvent) => {
    e.preventDefault();
    if (dragIndex === null || dragIndex === i) return;
    const next = [...visibleKeys];
    const [moved] = next.splice(dragIndex, 1);
    next.splice(i, 0, moved);
    onChange(next);
    setDragIndex(i);
  };

  return (
    <div className="relative" ref={ref}>
      <Button onClick={() => setOpen((o) => !o)}>
        <Columns3 size={14} /> Columns
      </Button>
      {open && (
        <div className="absolute right-0 z-30 mt-1 w-64 rounded-md border border-slate-200 bg-white shadow-lg dark:border-slate-700 dark:bg-slate-800">
          <div className="flex items-center justify-between border-b border-slate-200 px-3 py-2 dark:border-slate-700">
            <span className="text-[11px] font-semibold uppercase tracking-wide text-slate-400">
              Shown · drag to reorder
            </span>
            <button
              onClick={onReset}
              className="inline-flex items-center gap-1 text-[11px] font-medium text-sky-600 hover:underline dark:text-sky-400"
            >
              <RotateCcw size={11} /> Reset
            </button>
          </div>

          <div ref={shownRef} className="max-h-64 overflow-auto py-1">
            {shown.map((key, i) => (
              <div
                key={String(key)}
                draggable
                onDragStart={() => setDragIndex(i)}
                onDragOver={onDragOver(i)}
                onDragEnd={() => setDragIndex(null)}
                className={clsx(
                  "flex items-center gap-1.5 px-2 py-1.5 text-xs text-slate-700 dark:text-slate-200",
                  dragIndex === i
                    ? "bg-sky-50 dark:bg-sky-950/40"
                    : "hover:bg-slate-50 dark:hover:bg-slate-700/40",
                )}
              >
                <GripVertical
                  size={13}
                  className="shrink-0 cursor-grab text-slate-400 active:cursor-grabbing"
                />
                <span className="flex-1 truncate">{labelOf(key)}</span>
                <button
                  onClick={() => hide(key)}
                  disabled={visibleKeys.length === 1}
                  title="Hide column"
                  className="rounded p-0.5 text-slate-400 hover:bg-slate-200 hover:text-slate-600 disabled:opacity-30 dark:hover:bg-slate-600"
                >
                  <X size={13} />
                </button>
              </div>
            ))}
          </div>

          {hidden.length > 0 && (
            <>
              <div className="border-t border-slate-200 px-3 py-1.5 text-[11px] font-semibold uppercase tracking-wide text-slate-400 dark:border-slate-700">
                Add column
              </div>
              <div className="max-h-40 overflow-auto pb-1">
                {hidden.map((col) => (
                  <button
                    key={String(col.key)}
                    onClick={() => show(col.key)}
                    className="flex w-full items-center gap-1.5 px-2 py-1.5 text-left text-xs text-slate-600 hover:bg-slate-50 dark:text-slate-300 dark:hover:bg-slate-700/40"
                  >
                    <Plus size={13} className="shrink-0 text-slate-400" />
                    <span className="truncate">{col.label}</span>
                  </button>
                ))}
              </div>
            </>
          )}
        </div>
      )}
    </div>
  );
}
