import { useEffect, useState } from "react";
import { save } from "@tauri-apps/plugin-dialog";
import { Download, CheckCircle2, XCircle } from "lucide-react";
import { toast } from "sonner";
import { api, withToast } from "../../lib/api";
import type { ExportPreview } from "../../lib/types";
import { fmtFreq } from "../../lib/constants";
import { Modal } from "../overlays";
import { Button, Spinner, Badge } from "../ui";

export function ExportDialog({
  open,
  onClose,
  codeplugId,
  codeplugName,
  onExported,
}: {
  open: boolean;
  onClose: () => void;
  codeplugId: number;
  codeplugName: string;
  onExported: () => void;
}) {
  const [preview, setPreview] = useState<ExportPreview | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [exporting, setExporting] = useState(false);

  useEffect(() => {
    if (!open) return;
    setPreview(null);
    setError(null);
    setLoading(true);
    api
      .exportPreview(codeplugId)
      .then((p) => setPreview(p))
      .catch((e) => setError(typeof e === "string" ? e : String(e)))
      .finally(() => setLoading(false));
  }, [open, codeplugId]);

  const isAnytone = preview?.export_format === "anytone_csv";

  const doExport = async () => {
    const safe = codeplugName.replace(/[^\w.-]+/g, "_");
    const path = await save({
      defaultPath: `${safe}.csv`,
      filters: [{ name: isAnytone ? "Anytone CSV" : "CHIRP CSV", extensions: ["csv"] }],
    });
    if (!path) return;
    setExporting(true);
    const count = await withToast(api.generateCodeplug(codeplugId, path), {
      error: "Export failed",
    });
    setExporting(false);
    if (count !== undefined) {
      const fileName = path.split("/").pop() ?? "";
      const base = fileName.replace(/\.csv$/i, "");
      toast.success(
        isAnytone
          ? `Exported ${count} channel${count === 1 ? "" : "s"} to ${base}_Channels.csv + ${base}_TalkGroups.csv`
          : `Exported ${count} channel${count === 1 ? "" : "s"} to ${fileName}`,
      );
      onExported();
      onClose();
    }
  };

  return (
    <Modal
      open={open}
      onClose={onClose}
      title={`Export “${codeplugName}”`}
      width="max-w-3xl"
    >
      <div className="flex flex-col overflow-hidden">
        {loading ? (
          <div className="flex justify-center py-16">
            <Spinner className="h-6 w-6" />
          </div>
        ) : error ? (
          <div className="px-5 py-10 text-center">
            <XCircle size={32} className="mx-auto mb-2 text-red-400" />
            <p className="text-xs text-red-500">{error}</p>
          </div>
        ) : preview ? (
          <>
            <div className="flex flex-wrap items-center gap-4 border-b border-slate-200 px-5 py-3 text-xs dark:border-slate-700">
              <span className="text-slate-500 dark:text-slate-400">
                Target radio:{" "}
                <span className="font-semibold text-slate-800 dark:text-slate-100">
                  {preview.radio_model}
                </span>
              </span>
              <span className="inline-flex items-center gap-1 text-emerald-600 dark:text-emerald-400">
                <CheckCircle2 size={14} /> {preview.included_count} included
              </span>
              <span className="inline-flex items-center gap-1 text-slate-400">
                <XCircle size={14} /> {preview.excluded_count} excluded
              </span>
              {isAnytone && (
                <Badge className="bg-sky-100 text-sky-700 dark:bg-sky-950 dark:text-sky-300">
                  DMR-native · writes _Channels.csv + _TalkGroups.csv
                </Badge>
              )}
            </div>

            <div className="max-h-[50vh] overflow-auto">
              {preview.rows.length === 0 ? (
                <div className="px-5 py-10 text-center text-xs text-slate-400">
                  This codeplug has no channels. Assign channel lists with
                  channels before exporting.
                </div>
              ) : (
                <table className="w-full text-left text-[11px]">
                  <thead className="sticky top-0 bg-slate-100 text-[10px] uppercase tracking-wide text-slate-500 dark:bg-slate-900 dark:text-slate-400">
                    <tr>
                      <th className="px-3 py-1.5 font-semibold">Channel</th>
                      <th className="px-3 py-1.5 font-semibold">RX Freq</th>
                      <th className="px-3 py-1.5 font-semibold">Mode</th>
                      <th className="px-3 py-1.5 font-semibold">Status</th>
                    </tr>
                  </thead>
                  <tbody className="divide-y divide-slate-100 dark:divide-slate-800">
                    {preview.rows.map((r, i) => (
                      <tr
                        key={`${r.channel_id}-${i}`}
                        className={
                          r.included
                            ? "text-slate-700 dark:text-slate-200"
                            : "text-slate-400 dark:text-slate-500"
                        }
                      >
                        <td className="px-3 py-1.5">{r.name || "—"}</td>
                        <td className="px-3 py-1.5 font-mono">
                          {fmtFreq(r.rx_freq)}
                        </td>
                        <td className="px-3 py-1.5">{r.mode}</td>
                        <td className="px-3 py-1.5">
                          {r.included ? (
                            <Badge className="bg-emerald-100 text-emerald-700 dark:bg-emerald-950 dark:text-emerald-300">
                              Included
                            </Badge>
                          ) : (
                            <span
                              className="cursor-help text-amber-600 dark:text-amber-400"
                              title={r.reason ?? ""}
                            >
                              Excluded — {r.reason}
                            </span>
                          )}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              )}
            </div>
          </>
        ) : null}

        <div className="flex items-center justify-end gap-2 border-t border-slate-200 px-4 py-3 dark:border-slate-700">
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button
            variant="primary"
            onClick={doExport}
            disabled={
              exporting ||
              !preview ||
              preview.included_count === 0 ||
              !!error
            }
          >
            <Download size={14} />
            {exporting ? "Exporting…" : `Export ${preview?.included_count ?? 0} Channels`}
          </Button>
        </div>
      </div>
    </Modal>
  );
}
