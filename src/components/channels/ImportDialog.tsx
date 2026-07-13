import { useState, useEffect, type ReactNode } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { Upload, FileText } from "lucide-react";
import { api, withToast } from "../../lib/api";
import type { ImportPreview, ImportPreviewRow } from "../../lib/types";
import { Modal } from "../overlays";
import { Button, Spinner } from "../ui";
import { fmtFreq, fmtOffset } from "../../lib/constants";

// A checkmark for boolean fields, dim dot when false.
const yn = (b: boolean): ReactNode =>
  b ? (
    <span className="font-semibold text-emerald-600 dark:text-emerald-400">✓</span>
  ) : (
    <span className="text-slate-300 dark:text-slate-600">·</span>
  );

const tone = (n: number | null): string => (n != null ? n.toFixed(1) : "");

type Col = {
  label: string;
  render: (r: ImportPreviewRow) => ReactNode;
  mono?: boolean;
  center?: boolean;
};

// Every field that will be imported, in a sensible left-to-right order.
const COLUMNS: Col[] = [
  { label: "Name", render: (r) => r.name_long || r.name_short },
  { label: "Short", render: (r) => r.name_short },
  { label: "Call", render: (r) => r.callsign },
  { label: "RX", mono: true, render: (r) => fmtFreq(r.rx_freq) },
  { label: "TX", mono: true, render: (r) => (r.tx_freq != null ? fmtFreq(r.tx_freq) : "") },
  { label: "Dup", center: true, render: (r) => r.duplex },
  { label: "Offset", mono: true, render: (r) => fmtOffset(r.offset) },
  { label: "Band", render: (r) => r.band },
  { label: "Mode", render: (r) => r.mode },
  { label: "Tone", render: (r) => r.tone_mode },
  { label: "CTCSS↑", mono: true, render: (r) => tone(r.ctcss_uplink) },
  { label: "CTCSS↓", mono: true, render: (r) => tone(r.ctcss_downlink) },
  { label: "DMR CC", center: true, render: (r) => (r.dmr_color_code != null ? r.dmr_color_code : "") },
  { label: "D-STAR", center: true, render: (r) => yn(r.dstar_capable) },
  { label: "YSF", center: true, render: (r) => yn(r.ysf_capable) },
  { label: "NXDN", center: true, render: (r) => yn(r.nxdn_capable) },
  { label: "P25", center: true, render: (r) => yn(r.p25_capable) },
  { label: "P25 NAC", center: true, render: (r) => r.p25_nac ?? "" },
  { label: "M17", center: true, render: (r) => yn(r.m17_capable) },
  { label: "TETRA", center: true, render: (r) => yn(r.tetra_capable) },
  { label: "AllStar", mono: true, render: (r) => r.allstar_node ?? "" },
  { label: "EchoLink", mono: true, render: (r) => r.echolink_node ?? "" },
  { label: "IRLP", mono: true, render: (r) => r.irlp_node ?? "" },
  { label: "Wires-X", mono: true, render: (r) => r.wires_node ?? "" },
  { label: "Use", render: (r) => r.use_type ?? "" },
  { label: "Status", render: (r) => r.operational_status ?? "" },
  { label: "City", render: (r) => r.city ?? "" },
  { label: "County", render: (r) => r.county ?? "" },
  { label: "ST", center: true, render: (r) => r.state ?? "" },
  { label: "Country", render: (r) => r.country ?? "" },
  { label: "Lat", mono: true, render: (r) => (r.latitude != null ? r.latitude.toFixed(4) : "") },
  { label: "Lon", mono: true, render: (r) => (r.longitude != null ? r.longitude.toFixed(4) : "") },
  { label: "ARES", center: true, render: (r) => yn(r.ares) },
  { label: "RACES", center: true, render: (r) => yn(r.races) },
  { label: "SKYWARN", center: true, render: (r) => yn(r.skywarn) },
  { label: "CANWARN", center: true, render: (r) => yn(r.canwarn) },
  { label: "Notes", render: (r) => r.notes ?? "" },
];

export function ImportDialog({
  open: isOpen,
  onClose,
  onImported,
}: {
  open: boolean;
  onClose: () => void;
  onImported: () => void;
}) {
  const [path, setPath] = useState<string | null>(null);
  const [preview, setPreview] = useState<ImportPreview | null>(null);
  // Which importer this file routes to: a RepeaterBook CSV/JSON export, or a
  // native 73plug channel backup (a previously exported selection).
  const [kind, setKind] = useState<"csv" | "rb-json" | "native">("csv");
  const [loading, setLoading] = useState(false);
  const [importing, setImporting] = useState(false);

  // Reset state each time the dialog opens.
  useEffect(() => {
    if (isOpen) {
      setPath(null);
      setPreview(null);
      setKind("csv");
    }
  }, [isOpen]);

  // RepeaterBook exports come as CSV or (Plus) JSON; route by file extension.
  const isJson = (p: string) => p.toLowerCase().endsWith(".json");

  const pickFile = async () => {
    const selected = await open({
      multiple: false,
      filters: [
        { name: "Channel file (RepeaterBook export or 73plug backup)", extensions: ["csv", "json"] },
      ],
    });
    if (typeof selected !== "string") return;
    setPath(selected);
    setLoading(true);

    // A .json file may be a RepeaterBook export or our own native backup; probe
    // it so we use the lossless native importer for our own files.
    let nextKind: "csv" | "rb-json" | "native" = "csv";
    if (isJson(selected)) {
      nextKind = (await api.isChannelBackup(selected).catch(() => false))
        ? "native"
        : "rb-json";
    }
    setKind(nextKind);

    const pv = await withToast(
      nextKind === "native"
        ? api.previewChannelImport(selected)
        : nextKind === "rb-json"
          ? api.previewJsonImport(selected)
          : api.previewCsvImport(selected),
      { error: "Could not parse file" },
    );
    setLoading(false);
    if (pv) setPreview(pv);
  };

  const confirmImport = async () => {
    if (!path) return;
    setImporting(true);
    const summary = await withToast(
      kind === "native"
        ? api.importChannels(path)
        : kind === "rb-json"
          ? api.importJson(path)
          : api.importCsv(path),
      { error: "Import failed" },
    );
    setImporting(false);
    if (summary) {
      const { toast } = await import("sonner");
      toast.success(
        `Imported ${summary.added} channel${summary.added === 1 ? "" : "s"}` +
          (summary.updated ? ` · ${summary.updated} updated` : "") +
          (summary.skipped ? ` · ${summary.skipped} skipped (duplicates)` : ""),
      );
      onImported();
      onClose();
    }
  };

  const truncated = preview ? preview.rows.length < preview.total : false;

  return (
    <Modal open={isOpen} onClose={onClose} title="Import Channels" width="max-w-6xl">
      <div className="flex flex-1 flex-col overflow-hidden">
        <div className="flex items-center justify-between gap-3 border-b border-slate-200 px-4 py-2.5 dark:border-slate-700">
          <div className="flex min-w-0 items-center gap-2 text-xs text-slate-500 dark:text-slate-400">
            <FileText size={14} className="shrink-0" />
            <span className="truncate">
              {path ??
                "Select a RepeaterBook CSV/JSON export or a 73plug channel backup to preview."}
            </span>
          </div>
          <Button onClick={pickFile}>
            <Upload size={14} /> {path ? "Choose Different File" : "Choose File"}
          </Button>
        </div>

        <div className="flex min-h-0 flex-1 flex-col p-4">
          {loading ? (
            <div className="flex justify-center py-16">
              <Spinner className="h-6 w-6" />
            </div>
          ) : preview ? (
            <>
              <p className="mb-2 shrink-0 text-xs text-slate-500 dark:text-slate-400">
                {kind === "native" ? "73plug channel backup. " : ""}
                {truncated
                  ? `Showing first ${preview.rows.length} of ${preview.total} parsed rows.`
                  : `Showing all ${preview.total} parsed rows.`}{" "}
                Scroll right to see every field. New channels are added;{" "}
                {kind === "native"
                  ? "duplicates (same RepeaterBook ID, or same name + frequency for manual channels) are skipped. Full details (DCS, power, talkgroups) are restored from the backup even though the preview shows only the common fields."
                  : "existing channels (matched on RepeaterBook ID) are refreshed from this export — mode, tones, status and location are updated, while your custom names and any field edits you've made are preserved."}
              </p>
              <div className="min-h-0 flex-1 overflow-auto rounded-md border border-slate-200 dark:border-slate-700">
                <table className="text-left text-[11px]">
                  <thead className="sticky top-0 z-10 bg-slate-100 text-[10px] uppercase tracking-wide text-slate-500 dark:bg-slate-900 dark:text-slate-400">
                    <tr>
                      {COLUMNS.map((c) => (
                        <th
                          key={c.label}
                          className={`whitespace-nowrap px-2 py-1.5 font-semibold ${
                            c.center ? "text-center" : ""
                          }`}
                        >
                          {c.label}
                        </th>
                      ))}
                    </tr>
                  </thead>
                  <tbody className="divide-y divide-slate-100 dark:divide-slate-800">
                    {preview.rows.map((r, i) => (
                      <tr
                        key={i}
                        className="text-slate-700 hover:bg-slate-50 dark:text-slate-200 dark:hover:bg-slate-700/30"
                      >
                        {COLUMNS.map((c) => (
                          <td
                            key={c.label}
                            className={`whitespace-nowrap px-2 py-1 ${
                              c.mono ? "font-mono" : ""
                            } ${c.center ? "text-center" : ""} ${
                              c.label === "Notes" ? "max-w-[220px] truncate" : ""
                            }`}
                            title={c.label === "Notes" ? r.notes ?? "" : undefined}
                          >
                            {c.render(r)}
                          </td>
                        ))}
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
            {importing ? "Importing…" : `Import ${preview?.total ?? 0} Rows`}
          </Button>
        </div>
      </div>
    </Modal>
  );
}
