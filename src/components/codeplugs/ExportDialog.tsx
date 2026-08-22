import { useEffect, useState } from "react";
import { open as openDialog, save } from "@tauri-apps/plugin-dialog";
import { Download, CheckCircle2, XCircle, Ear, ShieldAlert } from "lucide-react";
import { toast } from "sonner";
import { api, withToast } from "../../lib/api";
import type { ExportPreview } from "../../lib/types";
import { fmtFreq } from "../../lib/constants";
import { Modal } from "../overlays";
import { Button, Spinner, Badge } from "../ui";
import { mediaWriteForFormat } from "../../lib/radioProgramming";

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
  // Media radios are programmed by patching a file the radio itself wrote, so
  // the user picks an EXISTING file to rewrite rather than naming a new one.
  // Same descriptor the Program dialog uses, so the two can't drift apart.
  const media = mediaWriteForFormat(preview?.export_format ?? null);

  const doExport = async () => {
    let path: string | null;
    if (media) {
      const picked = await openDialog({
        title: media.pickTitle,
        multiple: false,
        directory: false,
        filters: [{ name: media.filterName, extensions: media.extensions }],
      });
      path = typeof picked === "string" ? picked : null;
    } else {
      const safe = codeplugName.replace(/[^\w.-]+/g, "_");
      path = await save({
        defaultPath: `${safe}.csv`,
        filters: [{ name: isAnytone ? "Anytone CSV" : "CHIRP CSV", extensions: ["csv"] }],
      });
    }
    if (!path) return;
    setExporting(true);
    const written = await withToast(api.generateCodeplug(codeplugId, path), {
      error: "Export failed",
    });
    setExporting(false);
    if (written !== undefined) {
      // The file the exporter actually wrote, which is not always the one asked
      // for — a card export may have created and named it.
      const count = written.channels;
      const fileName = written.path.split("/").pop() ?? "";
      const base = fileName.replace(/\.csv$/i, "");
      const plural = count === 1 ? "" : "s";
      toast.success(
        media
          ? `Wrote ${count} channel${plural} into ${fileName}. ${media.after}`
          : isAnytone
            ? `Exported ${count} channel${plural} to ${base}_Channels.csv + ${base}_TalkGroups.csv`
            : `Exported ${count} channel${plural} to ${fileName}`,
      );
      // Settings the export left behind because they are outside the range the
      // schema declares. Separate from the success line: the file was written,
      // and this says what is not in it (#87).
      if (written.note) toast.warning(written.note, { duration: 12000 });
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
              {preview.receive_only_count > 0 && (
                <span className="inline-flex items-center gap-1 text-sky-600 dark:text-sky-400">
                  <Ear size={14} /> {preview.receive_only_count} receive-only
                </span>
              )}
              <span className="inline-flex items-center gap-1 text-slate-400">
                <XCircle size={14} /> {preview.excluded_count} excluded
              </span>
              {isAnytone && (
                <Badge className="bg-sky-100 text-sky-700 dark:bg-sky-950 dark:text-sky-300">
                  DMR-native · writes _Channels.csv + _TalkGroups.csv
                </Badge>
              )}
              {media && (
                <Badge className="bg-sky-100 text-sky-700 dark:bg-sky-950 dark:text-sky-300">
                  microSD · patches the radio’s own file in place
                </Badge>
              )}
            </div>

            {media && (
              <div className="border-b border-slate-200 bg-amber-50 px-5 py-2.5 text-[11px] leading-relaxed text-amber-900 dark:border-slate-700 dark:bg-amber-950/40 dark:text-amber-200">
                <span className="font-semibold">{media.before}</span> {media.preserves}
              </div>
            )}

            {/* The same fact from the sharing side (#75). The file this writes
                is a patched COPY of the operator's own radio file, so it is
                personal — which is exactly what makes it unsafe to post to a
                club list or attach to an issue without knowing. */}
            {media && (
              <div className="flex items-start gap-2 border-b border-slate-200 bg-slate-50 px-5 py-2.5 text-[11px] leading-relaxed text-slate-600 dark:border-slate-700 dark:bg-slate-800/60 dark:text-slate-300">
                <ShieldAlert
                  size={14}
                  className="mt-px shrink-0 text-slate-400 dark:text-slate-500"
                />
                <span>
                  <span className="font-semibold">
                    Sharing this file shares your radio.
                  </span>{" "}
                  It is a copy of the file your radio wrote, so it carries{" "}
                  {media.carries}. Fine for programming your own radio — think
                  twice before posting it to a club list or attaching it to a
                  bug report.
                </span>
              </div>
            )}

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
                          {r.included && r.receive_only ? (
                            <span
                              className="cursor-help text-sky-600 dark:text-sky-400"
                              title={r.reason ?? ""}
                            >
                              {r.reason}
                            </span>
                          ) : r.included ? (
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
            {exporting
              ? media
                ? "Writing…"
                : "Exporting…"
              : media
                ? `Write ${preview?.included_count ?? 0} Channels to microSD`
                : `Export ${preview?.included_count ?? 0} Channels`}
          </Button>
        </div>
      </div>
    </Modal>
  );
}
