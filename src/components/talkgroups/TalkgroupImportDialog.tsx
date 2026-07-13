import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { Upload, FileText } from "lucide-react";
import { api, withToast } from "../../lib/api";
import type { TalkgroupImportPreview } from "../../lib/types";
import { DMR_NETWORKS } from "../../lib/constants";
import { Modal } from "../overlays";
import { Button, Spinner, Select } from "../ui";

export function TalkgroupImportDialog({
  open: isOpen,
  onClose,
  onImported,
}: {
  open: boolean;
  onClose: () => void;
  onImported: () => void;
}) {
  const [path, setPath] = useState<string | null>(null);
  const [network, setNetwork] = useState(DMR_NETWORKS[0]);
  // "csv" = column-mapped CSV (needs a default network); "backup" = a lossless
  // 73plug talkgroup backup JSON (self-describing, no network needed).
  const [mode, setMode] = useState<"csv" | "backup">("csv");
  const [preview, setPreview] = useState<TalkgroupImportPreview | null>(null);
  const [loading, setLoading] = useState(false);
  const [importing, setImporting] = useState(false);

  // Reset each time the dialog opens.
  useEffect(() => {
    if (isOpen) {
      setPath(null);
      setPreview(null);
      setNetwork(DMR_NETWORKS[0]);
      setMode("csv");
    }
  }, [isOpen]);

  const runPreview = async (p: string, net: string, m: "csv" | "backup") => {
    setLoading(true);
    const pv = await withToast(
      m === "backup"
        ? api.previewTalkgroupBackup(p)
        : api.previewTalkgroupImport(p, net),
      { error: "Could not parse file" },
    );
    setLoading(false);
    if (pv) setPreview(pv);
  };

  const pickFile = async () => {
    const selected = await open({
      multiple: false,
      filters: [
        { name: "Talkgroup file (CSV or 73plug backup)", extensions: ["csv", "json"] },
      ],
    });
    if (typeof selected !== "string") return;
    setPath(selected);

    // A .json file is our own lossless backup; probe it so we use the backup
    // importer (self-describing, preserves notes + source) instead of the CSV path.
    const nextMode: "csv" | "backup" = selected.toLowerCase().endsWith(".json")
      ? (await api.isTalkgroupBackup(selected).catch(() => false))
        ? "backup"
        : "csv"
      : "csv";
    setMode(nextMode);
    await runPreview(selected, network, nextMode);
  };

  // Re-parse when the default network changes (CSV rows without their own network
  // column inherit it). Backups are self-describing, so this is a no-op for them.
  const onNetworkChange = async (net: string) => {
    setNetwork(net);
    if (path && mode === "csv") await runPreview(path, net, mode);
  };

  const confirmImport = async () => {
    if (!path) return;
    setImporting(true);
    const summary = await withToast(
      mode === "backup"
        ? api.importTalkgroupBackup(path)
        : api.importTalkgroups(path, network),
      { error: "Import failed" },
    );
    setImporting(false);
    if (summary) {
      const { toast } = await import("sonner");
      toast.success(
        `Imported ${summary.added} talkgroup${summary.added === 1 ? "" : "s"}` +
          (summary.skipped ? ` · ${summary.skipped} skipped (already exist)` : ""),
      );
      onImported();
      onClose();
    }
  };

  const truncated = preview ? preview.rows.length < preview.total : false;

  return (
    <Modal open={isOpen} onClose={onClose} title="Import Talkgroups" width="max-w-2xl">
      <div className="flex flex-1 flex-col overflow-hidden">
        <div className="flex flex-wrap items-center justify-between gap-3 border-b border-slate-200 px-4 py-2.5 dark:border-slate-700">
          <div className="flex min-w-0 items-center gap-2 text-xs text-slate-500 dark:text-slate-400">
            <FileText size={14} className="shrink-0" />
            <span className="truncate">
              {path
                ? mode === "backup"
                  ? `${path} · 73plug backup`
                  : path
                : "Select a talkgroup CSV (number + name, optional type/notes) or a 73plug backup."}
            </span>
          </div>
          <div className="flex items-center gap-2">
            {mode === "csv" && (
              <label className="flex items-center gap-1.5 text-[11px] text-slate-500 dark:text-slate-400">
                Network
                <Select value={network} onChange={(e) => onNetworkChange(e.target.value)}>
                  {DMR_NETWORKS.map((n) => (
                    <option key={n} value={n}>
                      {n}
                    </option>
                  ))}
                </Select>
              </label>
            )}
            <Button onClick={pickFile}>
              <Upload size={14} /> {path ? "Choose Different File" : "Choose File"}
            </Button>
          </div>
        </div>

        <div className="flex min-h-0 flex-1 flex-col p-4">
          {loading ? (
            <div className="flex justify-center py-16">
              <Spinner className="h-6 w-6" />
            </div>
          ) : preview ? (
            <>
              <p className="mb-2 shrink-0 text-xs text-slate-500 dark:text-slate-400">
                {truncated
                  ? `Showing first ${preview.rows.length} of ${preview.total} parsed talkgroups.`
                  : `Showing all ${preview.total} parsed talkgroups.`}{" "}
                New talkgroups are added; ones that already exist on the same
                network are skipped.
              </p>
              <div className="min-h-0 flex-1 overflow-auto rounded-md border border-slate-200 dark:border-slate-700">
                <table className="w-full text-left text-[11px]">
                  <thead className="sticky top-0 z-10 bg-slate-100 text-[10px] uppercase tracking-wide text-slate-500 dark:bg-slate-900 dark:text-slate-400">
                    <tr>
                      <th className="px-2 py-1.5 font-semibold">Number</th>
                      <th className="px-2 py-1.5 font-semibold">Name</th>
                      <th className="px-2 py-1.5 font-semibold">Network</th>
                      <th className="px-2 py-1.5 font-semibold">Type</th>
                      <th className="px-2 py-1.5 font-semibold">Notes</th>
                    </tr>
                  </thead>
                  <tbody className="divide-y divide-slate-100 dark:divide-slate-800">
                    {preview.rows.map((r, i) => (
                      <tr
                        key={i}
                        className="text-slate-700 hover:bg-slate-50 dark:text-slate-200 dark:hover:bg-slate-700/30"
                      >
                        <td className="px-2 py-1 font-mono tabular-nums">{r.tg_number}</td>
                        <td className="px-2 py-1 font-medium">{r.name}</td>
                        <td className="px-2 py-1">{r.network}</td>
                        <td className="px-2 py-1 text-slate-500">{r.call_type}</td>
                        <td className="max-w-[220px] truncate px-2 py-1 text-slate-400" title={r.notes ?? ""}>
                          {r.notes ?? ""}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            </>
          ) : (
            <div className="py-16 text-center text-xs text-slate-400">
              No file selected yet.
            </div>
          )}
        </div>

        <div className="flex items-center justify-end gap-2 border-t border-slate-200 px-4 py-3 dark:border-slate-700">
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button
            variant="primary"
            onClick={confirmImport}
            disabled={!preview || importing || preview.total === 0}
          >
            {importing ? "Importing…" : `Import ${preview?.total ?? 0} Talkgroups`}
          </Button>
        </div>
      </div>
    </Modal>
  );
}
