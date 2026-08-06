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
  // Channels ticked for import, by name. Everything starts ticked; unticking
  // is how you leave out, say, the GMRS repeater pairs.
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const [createList, setCreateList] = useState(true);
  const [listName, setListName] = useState("");
  const [importing, setImporting] = useState(false);

  useEffect(() => {
    if (!open) return;
    setSelectedId(null);
    setPicked(new Set());
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
          setPicked(new Set(l[0].channels.map((c) => c.name)));
        }
      })
      .finally(() => setLoading(false));
  }, [open]);

  const selected = useMemo(
    () => lists.find((l) => l.id === selectedId) ?? null,
    [lists, selectedId],
  );

  // Switching service retargets the channel-list name, unless the user has
  // typed one of their own, and starts the new list fully ticked.
  const select = (list: StandardListInfo) => {
    const previous = lists.find((l) => l.id === selectedId);
    if (!previous || listName === previous.name) setListName(list.name);
    setSelectedId(list.id);
    setPicked(new Set(list.channels.map((c) => c.name)));
  };

  const toggle = (name: string) =>
    setPicked((prev) => {
      const next = new Set(prev);
      if (!next.delete(name)) next.add(name);
      return next;
    });

  const allPicked = !!selected && picked.size === selected.channels.length;
  const nonePicked = picked.size === 0;
  const toggleAll = () =>
    setPicked(
      allPicked || !selected
        ? new Set()
        : new Set(selected.channels.map((c) => c.name)),
    );

  const doImport = async () => {
    if (!selected || nonePicked) return;
    setImporting(true);
    const summary = await withToast(
      api.importStandardList(
        selected.id,
        createList,
        createList ? listName.trim() || selected.name : null,
        allPicked ? null : [...picked],
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
                    {picked.size} of {selected.channel_count} selected ·{" "}
                    {selected.bands.join(", ")}
                  </span>
                </p>
                <div className="min-h-0 flex-1 overflow-auto">
                  <table className="w-full text-left text-[11px]">
                    <thead className="sticky top-0 z-10 bg-slate-100 text-[10px] uppercase tracking-wide text-slate-500 dark:bg-slate-900 dark:text-slate-400">
                      <tr>
                        <th className="w-8 px-3 py-1.5">
                          <input
                            type="checkbox"
                            aria-label="Select all channels"
                            className="h-3.5 w-3.5 rounded border-slate-300 text-sky-600 focus:ring-sky-500 dark:border-slate-600 dark:bg-slate-900"
                            checked={allPicked}
                            ref={(el) => {
                              if (el) el.indeterminate = !allPicked && !nonePicked;
                            }}
                            onChange={toggleAll}
                          />
                        </th>
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
                          onClick={() => toggle(c.name)}
                          className={`cursor-pointer text-slate-700 hover:bg-slate-50 dark:text-slate-200 dark:hover:bg-slate-700/30 ${
                            picked.has(c.name) ? "" : "opacity-40"
                          }`}
                        >
                          <td className="px-3 py-1">
                            <input
                              type="checkbox"
                              aria-label={c.name}
                              className="h-3.5 w-3.5 rounded border-slate-300 text-sky-600 focus:ring-sky-500 dark:border-slate-600 dark:bg-slate-900"
                              checked={picked.has(c.name)}
                              onChange={() => toggle(c.name)}
                              onClick={(e) => e.stopPropagation()}
                            />
                          </td>
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
            disabled={!selected || importing || nonePicked}
          >
            <BookMarked size={14} />
            {importing
              ? "Adding…"
              : `Add ${picked.size} Channel${picked.size === 1 ? "" : "s"}`}
          </Button>
        </div>
      </div>
    </Modal>
  );
}
