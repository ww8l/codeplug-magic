import { useEffect, useMemo, useState } from "react";
import { toast } from "sonner";
import { BookMarked, Radio } from "lucide-react";
import { api, withToast } from "../../lib/api";
import { fmtFreq } from "../../lib/constants";
import type { StandardListInfo } from "../../lib/types";
import { Modal } from "../overlays";
import { Button, TextInput, Spinner, Badge } from "../ui";

// The regulated services have fixed channel plans, so the catalog is static
// data in the backend — pick a service, see exactly what lands, import it.
export function StandardListDialog({
  open,
  onClose,
  onImported,
}: {
  open: boolean;
  onClose: () => void;
  onImported: () => void;
}) {
  const [lists, setLists] = useState<StandardListInfo[]>([]);
  const [loading, setLoading] = useState(false);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [createList, setCreateList] = useState(true);
  const [listName, setListName] = useState("");
  const [importing, setImporting] = useState(false);

  useEffect(() => {
    if (!open) return;
    setSelectedId(null);
    setCreateList(true);
    setListName("");
    setLoading(true);
    api
      .listStandardLists()
      .then((l) => {
        setLists(l);
        // Land on the first service so the dialog opens showing something.
        if (l.length) {
          setSelectedId(l[0].id);
          setListName(l[0].name);
        }
      })
      .finally(() => setLoading(false));
  }, [open]);

  const selected = useMemo(
    () => lists.find((l) => l.id === selectedId) ?? null,
    [lists, selectedId],
  );

  // Switching service retargets the channel-list name, unless the user has
  // typed one of their own.
  const select = (list: StandardListInfo) => {
    const previous = lists.find((l) => l.id === selectedId);
    if (!previous || listName === previous.name) setListName(list.name);
    setSelectedId(list.id);
  };

  const doImport = async () => {
    if (!selected) return;
    setImporting(true);
    const summary = await withToast(
      api.importStandardList(
        selected.id,
        createList,
        createList ? listName.trim() || selected.name : null,
      ),
      { error: "Could not import that list" },
    );
    setImporting(false);
    if (!summary) return;

    toast.success(
      `Added ${summary.added} ${selected.name} channel${
        summary.added === 1 ? "" : "s"
      }` +
        (summary.skipped ? ` · ${summary.skipped} already in library` : "") +
        (summary.list_name
          ? ` · ${summary.list_added} into “${summary.list_name}”`
          : ""),
    );
    onImported();
    onClose();
  };

  return (
    <Modal
      open={open}
      onClose={onClose}
      title="Add a standard channel list"
      width="max-w-5xl"
    >
      {loading ? (
        <div className="flex justify-center py-16">
          <Spinner className="h-6 w-6" />
        </div>
      ) : (
        <div className="flex min-h-0 flex-1">
          {/* Service picker */}
          <div className="w-64 shrink-0 overflow-y-auto border-r border-slate-200 dark:border-slate-700">
            {lists.map((l) => (
              <button
                key={l.id}
                onClick={() => select(l)}
                className={`w-full border-b border-slate-100 px-3 py-2.5 text-left last:border-b-0 dark:border-slate-700/60 ${
                  l.id === selectedId
                    ? "bg-sky-50 dark:bg-sky-950/40"
                    : "hover:bg-slate-50 dark:hover:bg-slate-700/40"
                }`}
              >
                <span className="flex items-center gap-1.5 text-xs font-medium text-slate-800 dark:text-slate-100">
                  <Radio size={13} className="shrink-0 text-slate-400" />
                  {l.name}
                  <Badge>{l.channel_count}</Badge>
                </span>
                <span className="mt-0.5 block text-[11px] leading-snug text-slate-500 dark:text-slate-400">
                  {l.full_name}
                </span>
              </button>
            ))}
          </div>

          {/* Preview */}
          <div className="flex min-w-0 flex-1 flex-col">
            {selected && (
              <>
                <p className="shrink-0 border-b border-slate-200 px-4 py-2.5 text-[11px] leading-snug text-slate-500 dark:border-slate-700 dark:text-slate-400">
                  {selected.description}{" "}
                  <span className="text-slate-400">
                    {selected.channel_count} channels ·{" "}
                    {selected.bands.join(", ")}
                  </span>
                </p>
                <div className="min-h-0 flex-1 overflow-auto">
                  <table className="w-full text-left text-[11px]">
                    <thead className="sticky top-0 z-10 bg-slate-100 text-[10px] uppercase tracking-wide text-slate-500 dark:bg-slate-900 dark:text-slate-400">
                      <tr>
                        <th className="px-3 py-1.5 font-semibold">Name</th>
                        <th className="px-2 py-1.5 font-semibold">Short</th>
                        <th className="px-2 py-1.5 font-semibold">RX</th>
                        <th className="px-2 py-1.5 font-semibold">TX</th>
                        <th className="px-2 py-1.5 font-semibold">Mode</th>
                        <th className="px-2 py-1.5 font-semibold">Power</th>
                        <th className="px-3 py-1.5 font-semibold">Notes</th>
                      </tr>
                    </thead>
                    <tbody className="divide-y divide-slate-100 dark:divide-slate-800">
                      {selected.channels.map((c) => (
                        <tr
                          key={c.name}
                          className="text-slate-700 dark:text-slate-200"
                        >
                          <td className="whitespace-nowrap px-3 py-1">
                            {c.name}
                          </td>
                          <td className="whitespace-nowrap px-2 py-1 font-mono">
                            {c.name_short}
                          </td>
                          <td className="whitespace-nowrap px-2 py-1 font-mono">
                            {fmtFreq(c.rx_freq)}
                          </td>
                          <td className="whitespace-nowrap px-2 py-1 font-mono">
                            {c.tx_freq == null ? (
                              <span className="font-sans text-slate-400">
                                RX only
                              </span>
                            ) : (
                              fmtFreq(c.tx_freq)
                            )}
                          </td>
                          <td className="whitespace-nowrap px-2 py-1">
                            {c.mode}
                          </td>
                          <td className="whitespace-nowrap px-2 py-1">
                            {c.power ?? ""}
                          </td>
                          <td className="px-3 py-1 text-slate-500 dark:text-slate-400">
                            {c.notes}
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              </>
            )}
          </div>
        </div>
      )}

      <div className="flex shrink-0 flex-wrap items-center gap-3 border-t border-slate-200 px-4 py-3 dark:border-slate-700">
        <label className="flex items-center gap-2 text-xs text-slate-600 dark:text-slate-300">
          <input
            type="checkbox"
            className="h-3.5 w-3.5 rounded border-slate-300 text-sky-600 focus:ring-sky-500 dark:border-slate-600 dark:bg-slate-900"
            checked={createList}
            onChange={(e) => setCreateList(e.target.checked)}
          />
          Also create a channel list
        </label>
        {createList && (
          <TextInput
            className="w-44"
            value={listName}
            onChange={(e) => setListName(e.target.value)}
            placeholder={selected?.name ?? "List name"}
          />
        )}
        <span className="text-[11px] text-slate-400">
          Channels already in your library are skipped, not duplicated.
        </span>
        <div className="ml-auto flex items-center gap-2">
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button
            variant="primary"
            onClick={doImport}
            disabled={!selected || importing}
          >
            <BookMarked size={14} />
            {importing
              ? "Adding…"
              : `Add ${selected?.channel_count ?? 0} Channels`}
          </Button>
        </div>
      </div>
    </Modal>
  );
}
