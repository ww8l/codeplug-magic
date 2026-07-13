import { useEffect, useMemo, useState } from "react";
import { Search, Plus, Check } from "lucide-react";
import clsx from "clsx";
import { Modal } from "../overlays";
import { TextInput, Badge } from "../ui";

export interface AssignItem {
  id: number;
  name: string;
  subtitle?: string | null;
  count?: number;
}

/** Generic "add from a list of available items" picker (channel/scan lists). */
export function AssignModal({
  open,
  onClose,
  title,
  items,
  emptyText,
  onAdd,
}: {
  open: boolean;
  onClose: () => void;
  title: string;
  items: AssignItem[];
  emptyText: string;
  onAdd: (id: number) => Promise<boolean>;
}) {
  const [search, setSearch] = useState("");
  const [added, setAdded] = useState<Set<number>>(new Set());
  const [pending, setPending] = useState<number | null>(null);

  useEffect(() => {
    if (open) {
      setSearch("");
      setAdded(new Set());
    }
  }, [open]);

  const filtered = useMemo(() => {
    const q = search.trim().toLowerCase();
    if (!q) return items;
    return items.filter((i) =>
      `${i.name} ${i.subtitle ?? ""}`.toLowerCase().includes(q),
    );
  }, [items, search]);

  const add = async (id: number) => {
    setPending(id);
    const ok = await onAdd(id);
    setPending(null);
    if (ok) setAdded((s) => new Set(s).add(id));
  };

  return (
    <Modal open={open} onClose={onClose} title={title} width="max-w-lg">
      <div className="flex flex-col gap-3 p-4">
        <div className="relative">
          <Search
            size={13}
            className="pointer-events-none absolute left-2 top-1/2 -translate-y-1/2 text-slate-400"
          />
          <TextInput
            autoFocus
            className="w-full pl-7"
            placeholder="Search…"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
          />
        </div>

        <div className="max-h-72 min-h-[6rem] overflow-auto rounded-md border border-slate-200 dark:border-slate-700">
          {filtered.length === 0 ? (
            <div className="px-3 py-8 text-center text-xs text-slate-400">
              {emptyText}
            </div>
          ) : (
            filtered.map((i) => {
              const isAdded = added.has(i.id);
              return (
                <div
                  key={i.id}
                  className="flex items-center justify-between gap-3 border-b border-slate-100 px-3 py-2 last:border-b-0 dark:border-slate-700/60"
                >
                  <div className="min-w-0">
                    <div className="flex items-center gap-1.5 truncate text-xs font-medium text-slate-800 dark:text-slate-100">
                      {i.name}
                      {i.count != null && <Badge>{i.count}</Badge>}
                    </div>
                    {i.subtitle && (
                      <div className="truncate text-[11px] text-slate-400">
                        {i.subtitle}
                      </div>
                    )}
                  </div>
                  <button
                    onClick={() => add(i.id)}
                    disabled={isAdded || pending === i.id}
                    className={clsx(
                      "inline-flex shrink-0 items-center gap-1 rounded-md px-2 py-1 text-[11px] font-medium transition-colors",
                      isAdded
                        ? "text-emerald-600 dark:text-emerald-400"
                        : "border border-slate-300 text-slate-600 hover:bg-slate-50 dark:border-slate-600 dark:text-slate-200 dark:hover:bg-slate-700",
                    )}
                  >
                    {isAdded ? (
                      <>
                        <Check size={12} /> Added
                      </>
                    ) : (
                      <>
                        <Plus size={12} /> Add
                      </>
                    )}
                  </button>
                </div>
              );
            })
          )}
        </div>
      </div>
    </Modal>
  );
}
