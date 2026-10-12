import { useEffect, useMemo, useState } from "react";
import { Search } from "lucide-react";
import { Modal } from "../overlays";
import { Button, TextInput, Spinner } from "../ui";
import { api, withToast } from "../../lib/api";
import type { Channel } from "../../lib/types";

const channelName = (c: Channel) =>
  c.name_long || c.name_short || c.callsign || `CH ${c.id}`;

/**
 * Choose, channel by channel, which of a codeplug's channels go in the radio's
 * one user memory group (the FTM-300's M-GRP). Membership is per codeplug.
 */
export function MemoryGroupModal({
  open,
  onClose,
  title,
  codeplugId,
  channelListIds,
  onChanged,
}: {
  open: boolean;
  onClose: () => void;
  title: string;
  codeplugId: number;
  channelListIds: number[];
  onChanged: (count: number) => void;
}) {
  const [channels, setChannels] = useState<Channel[]>([]);
  const [members, setMembers] = useState<Set<number>>(new Set());
  const [loading, setLoading] = useState(true);
  const [search, setSearch] = useState("");
  const [busy, setBusy] = useState(false);
  const listKey = channelListIds.join(",");

  useEffect(() => {
    if (!open) return;
    setSearch("");
    setLoading(true);
    (async () => {
      // The codeplug's channel pool: every assigned list, first-seen wins —
      // the same dedupe the export uses.
      const lists = await Promise.all(
        listKey ? listKey.split(",").map((id) => api.getChannelListChannels(Number(id))) : [],
      );
      const pool: Channel[] = [];
      const seen = new Set<number>();
      for (const list of lists) {
        for (const c of list) {
          if (!seen.has(c.id)) {
            seen.add(c.id);
            pool.push(c);
          }
        }
      }
      const ids = await api.getCodeplugMemoryGroup(codeplugId);
      setChannels(pool);
      setMembers(new Set(ids));
      setLoading(false);
    })();
  }, [open, codeplugId, listKey]);

  const filtered = useMemo(() => {
    const q = search.trim().toLowerCase();
    if (!q) return channels;
    return channels.filter((c) => channelName(c).toLowerCase().includes(q));
  }, [channels, search]);

  // Only channels still in the codeplug count; a removed list's channels stay
  // stored but are never written.
  const inPool = useMemo(
    () => channels.filter((c) => members.has(c.id)).length,
    [channels, members],
  );

  const set = async (batch: Channel[], member: boolean) => {
    if (batch.length === 0) return;
    setBusy(true);
    const ok = await withToast(
      api.setCodeplugMemoryGroup(codeplugId, batch.map((c) => c.id), member),
    );
    setBusy(false);
    if (ok === undefined) return;
    const next = new Set(members);
    for (const c of batch) {
      if (member) next.add(c.id);
      else next.delete(c.id);
    }
    setMembers(next);
    onChanged(channels.filter((c) => next.has(c.id)).length);
  };

  const shownIn = filtered.filter((c) => members.has(c.id));
  const shownOut = filtered.filter((c) => !members.has(c.id));

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
            placeholder="Search channels…"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
          />
        </div>

        <div className="flex items-center justify-between gap-2 text-[11px] text-slate-500 dark:text-slate-400">
          <span>
            {inPool} of {channels.length} channels in the group
          </span>
          <div className="flex gap-1.5">
            <Button variant="ghost" disabled={busy || loading || shownOut.length === 0} onClick={() => set(shownOut, true)}>
              {search.trim() ? "Check all shown" : "Check all"}
            </Button>
            <Button variant="ghost" disabled={busy || loading || shownIn.length === 0} onClick={() => set(shownIn, false)}>
              {search.trim() ? "Uncheck all shown" : "Uncheck all"}
            </Button>
          </div>
        </div>

        <div className="max-h-72 min-h-[6rem] overflow-auto rounded-md border border-slate-200 dark:border-slate-700">
          {loading ? (
            <div className="flex items-center justify-center py-10">
              <Spinner className="h-5 w-5" />
            </div>
          ) : filtered.length === 0 ? (
            <div className="px-3 py-8 text-center text-xs text-slate-400">
              {channels.length === 0 ? "This codeplug has no channels yet." : "No channels match your search."}
            </div>
          ) : (
            filtered.map((c) => (
              <label
                key={c.id}
                className="flex cursor-pointer items-center gap-2.5 border-b border-slate-100 px-3 py-2 last:border-b-0 hover:bg-slate-50 dark:border-slate-700/60 dark:hover:bg-slate-700/40"
              >
                <input
                  type="checkbox"
                  checked={members.has(c.id)}
                  disabled={busy}
                  onChange={(e) => set([c], e.target.checked)}
                  className="h-3.5 w-3.5 shrink-0 rounded border-slate-300 text-emerald-600 focus:ring-emerald-500 dark:border-slate-600"
                />
                <span className="min-w-0 flex-1 truncate text-xs font-medium text-slate-800 dark:text-slate-100">
                  {channelName(c)}
                </span>
                <span className="shrink-0 font-mono text-[11px] text-slate-400">{c.rx_freq.toFixed(4)}</span>
              </label>
            ))
          )}
        </div>
      </div>
    </Modal>
  );
}
