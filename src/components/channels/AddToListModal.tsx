import { useEffect, useState } from "react";
import { toast } from "sonner";
import { List, Plus } from "lucide-react";
import { api, withToast } from "../../lib/api";
import type { ChannelListSummary } from "../../lib/types";
import { Modal } from "../overlays";
import { Button, TextInput, Spinner, Badge } from "../ui";

export function AddToListModal({
  open,
  onClose,
  channelIds,
  onAdded,
}: {
  open: boolean;
  onClose: () => void;
  channelIds: number[];
  onAdded: () => void;
}) {
  const [lists, setLists] = useState<ChannelListSummary[]>([]);
  const [loading, setLoading] = useState(false);
  const [newName, setNewName] = useState("");
  const [alsoScan, setAlsoScan] = useState(false);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!open) return;
    setNewName("");
    setAlsoScan(false);
    setLoading(true);
    api
      .listChannelLists()
      .then(setLists)
      .finally(() => setLoading(false));
  }, [open]);

  const count = channelIds.length;

  // Report how many were newly added vs skipped as already-present.
  const finish = (added: number, listName: string) => {
    const dupes = count - added;
    toast.success(
      `Added ${added} to “${listName}”` +
        (dupes > 0 ? ` · ${dupes} already in list` : ""),
    );
    onAdded();
  };

  const addToExisting = async (list: ChannelListSummary) => {
    setBusy(true);
    const added = await withToast(
      api.addChannelsToList(list.id, channelIds),
      { error: "Could not add to list" },
    );
    setBusy(false);
    if (added !== undefined) finish(added, list.name);
  };

  const createAndAdd = async () => {
    const name = newName.trim();
    if (!name) return;
    setBusy(true);
    const list = await withToast(api.createChannelList(name, null), {
      error: "Could not create list",
    });
    if (!list) {
      setBusy(false);
      return;
    }
    const added = await withToast(api.addChannelsToList(list.id, channelIds), {
      error: "List created but adding failed",
    });
    // Optionally mirror the new list into a matching scan list with the same
    // name and the same channels.
    if (added !== undefined && alsoScan) {
      const scan = await withToast(api.createScanList(name, null), {
        error: "Channel list created, but the scan list could not be",
      });
      if (scan) {
        await withToast(api.addChannelsToScanList(scan.id, channelIds), {
          error: "Scan list created but adding channels failed",
        });
      }
    }
    setBusy(false);
    if (added !== undefined) finish(added, list.name);
  };

  return (
    <Modal
      open={open}
      onClose={onClose}
      title={`Add ${count} repeater${count === 1 ? "" : "s"} to a channel list`}
      width="max-w-md"
    >
      <div className="flex flex-col gap-4 p-4">
        {/* Create new */}
        <div>
          <h3 className="mb-1.5 text-[11px] font-semibold uppercase tracking-wide text-slate-400">
            Create new list
          </h3>
          <div className="flex gap-2">
            <TextInput
              className="flex-1"
              placeholder="New channel list name…"
              value={newName}
              onChange={(e) => setNewName(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && createAndAdd()}
            />
            <Button
              variant="primary"
              onClick={createAndAdd}
              disabled={busy || !newName.trim()}
            >
              <Plus size={14} /> Create &amp; Add
            </Button>
          </div>
          <label className="mt-2 flex items-center gap-2 text-xs text-slate-600 dark:text-slate-300">
            <input
              type="checkbox"
              className="h-3.5 w-3.5 rounded border-slate-300 text-sky-600 focus:ring-sky-500 dark:border-slate-600 dark:bg-slate-900"
              checked={alsoScan}
              onChange={(e) => setAlsoScan(e.target.checked)}
            />
            Also create a matching scan list with these channels
          </label>
        </div>

        {/* Existing */}
        <div>
          <h3 className="mb-1.5 text-[11px] font-semibold uppercase tracking-wide text-slate-400">
            Or add to existing
          </h3>
          {loading ? (
            <div className="flex justify-center py-8">
              <Spinner className="h-5 w-5" />
            </div>
          ) : lists.length === 0 ? (
            <p className="py-4 text-center text-xs text-slate-400">
              No channel lists yet — create one above.
            </p>
          ) : (
            <div className="max-h-64 overflow-auto rounded-md border border-slate-200 dark:border-slate-700">
              {lists.map((l) => (
                <button
                  key={l.id}
                  onClick={() => addToExisting(l)}
                  disabled={busy}
                  className="flex w-full items-center justify-between gap-2 border-b border-slate-100 px-3 py-2 text-left last:border-b-0 hover:bg-slate-50 disabled:opacity-50 dark:border-slate-700/60 dark:hover:bg-slate-700/40"
                >
                  <span className="flex items-center gap-1.5 truncate text-xs font-medium text-slate-800 dark:text-slate-100">
                    <List size={13} className="shrink-0 text-slate-400" />
                    {l.name}
                    <Badge>{l.channel_count}</Badge>
                  </span>
                  <span className="inline-flex shrink-0 items-center gap-1 text-[11px] font-medium text-sky-600 dark:text-sky-400">
                    <Plus size={12} /> Add
                  </span>
                </button>
              ))}
            </div>
          )}
        </div>
      </div>

      <div className="flex items-center justify-end border-t border-slate-200 px-4 py-3 dark:border-slate-700">
        <Button variant="ghost" onClick={onClose}>
          Done
        </Button>
      </div>
    </Modal>
  );
}
