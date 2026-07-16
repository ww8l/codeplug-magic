import { useState } from "react";
import { ChevronUp, ChevronDown, X, GripVertical } from "lucide-react";
import clsx from "clsx";
import type { Channel } from "../../lib/types";
import { fmtFreq } from "../../lib/constants";
import { channelLabel } from "./channelLabel";
import { Badge } from "../ui";

export function OrderedChannelTable({
  channels,
  onReorder,
  onRemove,
  onEdit,
}: {
  channels: Channel[];
  onReorder: (orderedIds: number[]) => void;
  onRemove: (channelId: number) => void;
  /** Open a row's channel for editing. Rows are only clickable when given. */
  onEdit?: (channel: Channel) => void;
}) {
  // Index of the row being dragged, and the row it's currently hovering over.
  const [dragIndex, setDragIndex] = useState<number | null>(null);
  const [overIndex, setOverIndex] = useState<number | null>(null);

  const move = (index: number, dir: -1 | 1) => {
    const target = index + dir;
    if (target < 0 || target >= channels.length) return;
    const ids = channels.map((c) => c.id);
    [ids[index], ids[target]] = [ids[target], ids[index]];
    onReorder(ids);
  };

  // Drop the dragged row at `to`, shifting the rest. Pulls the moved id out and
  // re-inserts it at the target position.
  const drop = (to: number) => {
    if (dragIndex === null || dragIndex === to) {
      setDragIndex(null);
      setOverIndex(null);
      return;
    }
    const ids = channels.map((c) => c.id);
    const [moved] = ids.splice(dragIndex, 1);
    ids.splice(to, 0, moved);
    setDragIndex(null);
    setOverIndex(null);
    onReorder(ids);
  };

  return (
    <table className="w-full text-xs">
      <thead className="sticky top-0 bg-slate-50 text-slate-500 dark:bg-slate-900/80 dark:text-slate-400">
        <tr className="border-b border-slate-200 dark:border-slate-700">
          <th className="w-6 px-1 py-1.5"></th>
          <th className="w-10 px-2 py-1.5 text-left font-medium">#</th>
          <th className="px-2 py-1.5 text-left font-medium">Channel</th>
          <th className="px-2 py-1.5 text-left font-medium">RX Freq</th>
          <th className="px-2 py-1.5 text-left font-medium">Band</th>
          <th className="px-2 py-1.5 text-left font-medium">Mode</th>
          <th className="w-24 px-2 py-1.5 text-right font-medium">Order</th>
          <th className="w-8 px-2 py-1.5"></th>
        </tr>
      </thead>
      <tbody>
        {channels.map((c, i) => (
          <tr
            key={c.id}
            draggable
            onDragStart={(e) => {
              setDragIndex(i);
              // Firefox requires data to be set for a drag to begin.
              e.dataTransfer.effectAllowed = "move";
              e.dataTransfer.setData("text/plain", String(c.id));
            }}
            onDragOver={(e) => {
              e.preventDefault();
              e.dataTransfer.dropEffect = "move";
              if (overIndex !== i) setOverIndex(i);
            }}
            onDrop={(e) => {
              e.preventDefault();
              drop(i);
            }}
            onDragEnd={() => {
              setDragIndex(null);
              setOverIndex(null);
            }}
            onClick={() => onEdit?.(c)}
            className={clsx(
              "border-b border-slate-100 dark:border-slate-700/60",
              onEdit && "cursor-pointer",
              dragIndex === i
                ? "opacity-40"
                : "hover:bg-slate-50 dark:hover:bg-slate-700/30",
              // Show where the row will land.
              overIndex === i &&
                dragIndex !== null &&
                dragIndex !== i &&
                (dragIndex > i
                  ? "border-t-2 border-t-sky-500"
                  : "border-b-2 border-b-sky-500"),
            )}
          >
            <td
              onClick={(e) => e.stopPropagation()}
              className="cursor-grab px-1 py-1.5 text-slate-300 hover:text-slate-500 active:cursor-grabbing dark:text-slate-600 dark:hover:text-slate-400"
              title="Drag to reorder"
            >
              <GripVertical size={14} />
            </td>
            <td className="px-2 py-1.5 tabular-nums text-slate-400">{i + 1}</td>
            <td className="px-2 py-1.5 font-medium text-slate-800 dark:text-slate-100">
              {channelLabel(c)}
            </td>
            <td className="px-2 py-1.5 tabular-nums text-slate-600 dark:text-slate-300">
              {fmtFreq(c.rx_freq)}
            </td>
            <td className="px-2 py-1.5">{c.band && <Badge>{c.band}</Badge>}</td>
            <td className="px-2 py-1.5 text-slate-600 dark:text-slate-300">
              {c.mode}
            </td>
            <td onClick={(e) => e.stopPropagation()} className="px-2 py-1.5">
              <div className="flex items-center justify-end gap-0.5">
                <button
                  onClick={() => move(i, -1)}
                  disabled={i === 0}
                  className="rounded p-0.5 text-slate-400 hover:bg-slate-200 disabled:opacity-30 dark:hover:bg-slate-600"
                >
                  <ChevronUp size={14} />
                </button>
                <button
                  onClick={() => move(i, 1)}
                  disabled={i === channels.length - 1}
                  className="rounded p-0.5 text-slate-400 hover:bg-slate-200 disabled:opacity-30 dark:hover:bg-slate-600"
                >
                  <ChevronDown size={14} />
                </button>
              </div>
            </td>
            <td onClick={(e) => e.stopPropagation()} className="px-2 py-1.5">
              <button
                onClick={() => onRemove(c.id)}
                title="Remove from list"
                className="rounded p-0.5 text-slate-400 hover:bg-red-100 hover:text-red-600 dark:hover:bg-red-950/50"
              >
                <X size={14} />
              </button>
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}
