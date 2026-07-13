import { useEffect, useMemo, useState } from "react";
import { Search, Plus, Check } from "lucide-react";
import clsx from "clsx";
import { api } from "../../lib/api";
import type { Channel } from "../../lib/types";
import { fmtFreq } from "../../lib/constants";
import { channelLabel } from "./channelLabel";
import { Modal } from "../overlays";
import { TextInput, Spinner, Badge } from "../ui";

export function ChannelPickerModal({
  open,
  onClose,
  existingIds,
  onAdd,
}: {
  open: boolean;
  onClose: () => void;
  existingIds: Set<number>;
  // Returns true when the channel was successfully added.
  onAdd: (channelId: number) => Promise<boolean>;
}) {
  const [channels, setChannels] = useState<Channel[]>([]);
  const [loading, setLoading] = useState(false);
  const [search, setSearch] = useState("");
  const [added, setAdded] = useState<Set<number>>(new Set());
  const [pending, setPending] = useState<number | null>(null);

  useEffect(() => {
    if (!open) return;
    setSearch("");
    setAdded(new Set());
    setLoading(true);
    api
      .listChannels()
      .then(setChannels)
      .finally(() => setLoading(false));
  }, [open]);

  const filtered = useMemo(() => {
    const q = search.trim().toLowerCase();
    return channels.filter((c) => {
      if (existingIds.has(c.id)) return false;
      if (!q) return true;
      return `${channelLabel(c)} ${c.callsign ?? ""} ${c.city ?? ""} ${c.state ?? ""}`
        .toLowerCase()
        .includes(q);
    });
  }, [channels, search, existingIds]);

  const add = async (id: number) => {
    setPending(id);
    const ok = await onAdd(id);
    setPending(null);
    if (ok) setAdded((s) => new Set(s).add(id));
  };

  return (
    <Modal open={open} onClose={onClose} title="Add Channels" width="max-w-2xl">
      <div className="flex flex-col gap-3 overflow-hidden p-4">
        <div className="relative">
          <Search
            size={13}
            className="pointer-events-none absolute left-2 top-1/2 -translate-y-1/2 text-slate-400"
          />
          <TextInput
            autoFocus
            className="w-full pl-7"
            placeholder="Search the Master Channel Table…"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
          />
        </div>

        <div className="max-h-80 min-h-[8rem] overflow-auto rounded-md border border-slate-200 dark:border-slate-700">
          {loading ? (
            <div className="flex items-center justify-center py-12">
              <Spinner className="h-5 w-5" />
            </div>
          ) : filtered.length === 0 ? (
            <div className="px-3 py-10 text-center text-xs text-slate-400">
              {channels.length === 0
                ? "No channels in the Master Channel Table yet."
                : "No matching channels available to add."}
            </div>
          ) : (
            filtered.map((c) => {
              const isAdded = added.has(c.id);
              return (
                <div
                  key={c.id}
                  className="flex items-center justify-between gap-3 border-b border-slate-100 px-3 py-2 last:border-b-0 dark:border-slate-700/60"
                >
                  <div className="min-w-0">
                    <div className="truncate text-xs font-medium text-slate-800 dark:text-slate-100">
                      {channelLabel(c)}
                    </div>
                    <div className="flex items-center gap-1.5 text-[11px] text-slate-400">
                      <span>{fmtFreq(c.rx_freq)} MHz</span>
                      {c.band && <Badge>{c.band}</Badge>}
                      {c.mode && <span>{c.mode}</span>}
                      {(c.city || c.state) && (
                        <span className="truncate">
                          {[c.city, c.state].filter(Boolean).join(", ")}
                        </span>
                      )}
                    </div>
                  </div>
                  <button
                    onClick={() => add(c.id)}
                    disabled={isAdded || pending === c.id}
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
