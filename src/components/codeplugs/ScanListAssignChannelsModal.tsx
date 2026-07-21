import { useEffect, useMemo, useState } from "react";
import { Search } from "lucide-react";
import clsx from "clsx";
import { Modal } from "../overlays";
import { Button, TextInput, Spinner } from "../ui";
import { api, withToast } from "../../lib/api";
import type { Channel, ScanListSummary } from "../../lib/types";

const channelName = (c: Channel) =>
  c.name_long || c.name_short || c.callsign || `CH ${c.id}`;

/**
 * Assign a codeplug's channels to ONE scan list — this is the radio's
 * per-channel "scan list" field (byte 0x1B). Each channel points at exactly
 * one scan list within the codeplug, so checking a channel already assigned
 * elsewhere MOVES it here. Membership in a scan list does NOT set this field:
 * an unchecked channel programs with no scan list, whatever lists it belongs
 * to.
 */
export function ScanListAssignChannelsModal({
  open,
  onClose,
  codeplugId,
  scanList,
  channelListIds,
  scanLists,
  onChanged,
}: {
  open: boolean;
  onClose: () => void;
  codeplugId: number;
  scanList: ScanListSummary | null;
  channelListIds: number[];
  scanLists: ScanListSummary[];
  onChanged: () => void;
}) {
  const [channels, setChannels] = useState<Channel[]>([]);
  // channel_id → assigned scan_list_id
  const [assigned, setAssigned] = useState<Map<number, number>>(new Map());
  const [loading, setLoading] = useState(true);
  const [search, setSearch] = useState("");
  const [pending, setPending] = useState<Set<number>>(new Set());
  const [bulking, setBulking] = useState(false);

  useEffect(() => {
    if (!open) return;
    setSearch("");
    setLoading(true);
    (async () => {
      // Codeplug channel pool: every assigned list's channels, first-seen wins
      // (mirrors the program builder's dedupe).
      const lists = await Promise.all(
        channelListIds.map((lid) => api.getChannelListChannels(lid)),
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
      const rows = await api.getCodeplugChannelScanLists(codeplugId);
      setChannels(pool);
      setAssigned(new Map(rows.map((r) => [r.channel_id, r.scan_list_id])));
      setLoading(false);
    })();
  }, [open, codeplugId, channelListIds]);

  const listNameById = useMemo(
    () => new Map(scanLists.map((l) => [l.id, l.name])),
    [scanLists],
  );

  const filtered = useMemo(() => {
    const q = search.trim().toLowerCase();
    if (!q) return channels;
    return channels.filter((c) => channelName(c).toLowerCase().includes(q));
  }, [channels, search]);

  const assignedHere = useMemo(
    () =>
      scanList
        ? [...assigned.values()].filter((id) => id === scanList.id).length
        : 0,
    [assigned, scanList],
  );
  // Bulk targets come from the filtered view, so a search narrows the action.
  const assignedShown = useMemo(
    () => filtered.filter((c) => assigned.get(c.id) === scanList?.id),
    [filtered, assigned, scanList],
  );
  const unassignedShown = useMemo(
    () => filtered.filter((c) => assigned.get(c.id) !== scanList?.id),
    [filtered, assigned, scanList],
  );

  /** Assign (or clear) a batch of channels, with one toast for the batch. */
  const bulkSet = async (batch: Channel[], checked: boolean) => {
    if (!scanList || batch.length === 0) return;
    setBulking(true);
    try {
      await withToast(
        (async () => {
          for (const c of batch) {
            if (checked) {
              await api.setChannelScanList(codeplugId, c.id, scanList.id);
            } else {
              await api.clearChannelScanList(codeplugId, c.id);
            }
          }
        })(),
      );
      setAssigned((prev) => {
        const next = new Map(prev);
        for (const c of batch) {
          if (checked) next.set(c.id, scanList.id);
          else next.delete(c.id);
        }
        return next;
      });
      onChanged();
    } finally {
      setBulking(false);
    }
  };

  const toggle = async (c: Channel, checked: boolean) => {
    if (!scanList) return;
    setPending((s) => new Set(s).add(c.id));
    const res = checked
      ? await withToast(api.setChannelScanList(codeplugId, c.id, scanList.id))
      : await withToast(api.clearChannelScanList(codeplugId, c.id));
    setPending((s) => {
      const n = new Set(s);
      n.delete(c.id);
      return n;
    });
    if (res === undefined) return;
    setAssigned((prev) => {
      const next = new Map(prev);
      if (checked) next.set(c.id, scanList.id);
      else next.delete(c.id);
      return next;
    });
    onChanged();
  };

  return (
    <Modal
      open={open}
      onClose={onClose}
      title={scanList ? `Assign channels — ${scanList.name}` : "Assign channels"}
      width="max-w-lg"
    >
      <div className="flex flex-col gap-3 p-4">
        <p className="text-[11px] leading-relaxed text-slate-500 dark:text-slate-400">
          Pick which of this codeplug's channels launch{" "}
          <span className="font-medium">{scanList?.name}</span> when Scan is
          pressed. A channel can point at only one scan list, so checking one
          that's assigned elsewhere moves it here. Being a member of a scan list
          doesn't set this — an unchecked channel launches no list at all.
        </p>

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
            {assignedHere} of {channels.length} channels launch this list
          </span>
          <div className="flex gap-1.5">
            <Button
              variant="ghost"
              disabled={bulking || loading || unassignedShown.length === 0}
              onClick={() => bulkSet(unassignedShown, true)}
            >
              {search.trim() ? "Check all shown" : "Check all"}
            </Button>
            <Button
              variant="ghost"
              disabled={bulking || loading || assignedShown.length === 0}
              onClick={() => bulkSet(assignedShown, false)}
            >
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
              {channels.length === 0
                ? "This codeplug has no channels yet."
                : "No channels match your search."}
            </div>
          ) : (
            filtered.map((c) => {
              const here = assigned.get(c.id) === scanList?.id;
              const other = assigned.get(c.id);
              const elsewhere =
                other != null && other !== scanList?.id
                  ? listNameById.get(other)
                  : undefined;
              return (
                <label
                  key={c.id}
                  className="flex cursor-pointer items-center gap-2.5 border-b border-slate-100 px-3 py-2 last:border-b-0 hover:bg-slate-50 dark:border-slate-700/60 dark:hover:bg-slate-700/40"
                >
                  <input
                    type="checkbox"
                    checked={here}
                    disabled={bulking || pending.has(c.id)}
                    onChange={(e) => toggle(c, e.target.checked)}
                    className="h-3.5 w-3.5 shrink-0 rounded border-slate-300 text-emerald-600 focus:ring-emerald-500 dark:border-slate-600"
                  />
                  <span className="min-w-0 flex-1 truncate text-xs font-medium text-slate-800 dark:text-slate-100">
                    {channelName(c)}
                  </span>
                  {elsewhere && (
                    <span
                      className={clsx(
                        "shrink-0 rounded px-1.5 py-0.5 text-[10px]",
                        "bg-slate-100 text-slate-500 dark:bg-slate-700 dark:text-slate-400",
                      )}
                    >
                      → {elsewhere}
                    </span>
                  )}
                </label>
              );
            })
          )}
        </div>
      </div>
    </Modal>
  );
}
