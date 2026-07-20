import { useEffect, useState } from "react";
import {
  RadioTower,
  RefreshCw,
  Search,
  DownloadCloud,
  Upload,
  CheckCircle2,
  XCircle,
  ShieldCheck,
  AlertTriangle,
  Usb,
  Undo2,
} from "lucide-react";
import { api } from "../../lib/api";
import type {
  DownloadResult,
  ExportPreview,
  PortInfo,
  CodeplugProgramReport,
  RadioIdent,
} from "../../lib/types";
import { Modal } from "../overlays";
import { Button, Spinner, Select } from "../ui";

/**
 * Direct UV-5R programming.
 *
 * Read: identify + download a full backup image. Write: `program_radio`
 * always downloads + backs up the radio first, then writes only the channel +
 * name regions and reads them back to verify. Settings/aux memory are
 * round-tripped untouched.
 */
/// This dialog is the UV-5R's, so it names the driver it dispatches to.
/// Threading `driver_key` through the model DTO instead is Chunk 3.7's job
/// (that's what `radio_models.programming_ui` is for).
const DRIVER_KEY = "baofeng_uv5r";

export function ProgramRadioDialog({
  open,
  onClose,
  codeplugId,
  codeplugName,
  isSupported,
  modelName,
}: {
  open: boolean;
  onClose: () => void;
  codeplugId: number;
  codeplugName: string;
  // Direct programming currently supports the Baofeng UV-5R only.
  isSupported: boolean;
  modelName: string;
}) {
  const [ports, setPorts] = useState<PortInfo[]>([]);
  const [port, setPort] = useState<string>("");
  const [preview, setPreview] = useState<ExportPreview | null>(null);
  const [busy, setBusy] = useState<
    null | "identify" | "download" | "program" | "restore"
  >(null);
  const [ident, setIdent] = useState<RadioIdent | null>(null);
  const [download, setDownload] = useState<DownloadResult | null>(null);
  const [program, setProgram] = useState<CodeplugProgramReport | null>(null);
  const [confirming, setConfirming] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refreshPorts = async () => {
    try {
      const list = await api.listSerialPorts();
      setPorts(list);
      const usb = list.find((p) => p.kind === "usb");
      setPort((cur) => cur || usb?.name || list[0]?.name || "");
    } catch (e) {
      setError(typeof e === "string" ? e : String(e));
    }
  };

  useEffect(() => {
    if (!open) return;
    setIdent(null);
    setDownload(null);
    setProgram(null);
    setConfirming(false);
    setError(null);
    setBusy(null);
    refreshPorts();
    if (isSupported) {
      api.exportPreview(codeplugId).then(setPreview).catch(() => setPreview(null));
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  const run = async (
    kind: "identify" | "download" | "program" | "restore",
    fn: () => Promise<void>,
  ) => {
    setError(null);
    setBusy(kind);
    try {
      await fn();
    } catch (e) {
      setError(typeof e === "string" ? e : String(e));
    } finally {
      setBusy(null);
    }
  };

  const doIdentify = () =>
    run("identify", async () => {
      setDownload(null);
      setProgram(null);
      setIdent(await api.identifyRadio(DRIVER_KEY, port));
    });

  const doDownload = () =>
    run("download", async () => {
      setProgram(null);
      setDownload(await api.downloadImage(DRIVER_KEY, port));
    });

  const doProgram = () =>
    run("program", async () => {
      setConfirming(false);
      setDownload(null);
      setIdent(null);
      setProgram(await api.programRadio(codeplugId, port));
    });

  const doRestore = async () => {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const defaultPath = await api.backupsDir().catch(() => undefined);
    const picked = await open({
      title: "Choose a backup .img to restore",
      multiple: false,
      defaultPath,
      filters: [{ name: "Radio backup image", extensions: ["img"] }],
    });
    if (typeof picked !== "string") return;
    await run("restore", async () => {
      setDownload(null);
      setProgram(null);
      setIdent(null);
      const res = await api.restoreImage(port, picked);
      const { toast } = await import("sonner");
      toast.success(
        `Restored ${res.bytes.toLocaleString()} bytes to the radio. Power-cycle it to check.`,
      );
    });
  };

  const writeCount = preview?.included_count ?? 0;
  const clearCount = Math.max(0, 128 - writeCount);
  const skipped = (preview?.rows ?? []).filter((r) => !r.included);

  return (
    <Modal
      open={open}
      onClose={onClose}
      title={`Program Radio — ${codeplugName}`}
      width="max-w-2xl"
    >
      <div className="flex flex-col">
        {!isSupported ? (
          <div className="px-5 py-10 text-center">
            <RadioTower size={32} className="mx-auto mb-2 text-slate-400" />
            <p className="text-sm font-medium text-slate-700 dark:text-slate-200">
              Direct programming isn’t available for {modelName}.
            </p>
            <p className="mt-1 text-xs text-slate-500 dark:text-slate-400">
              The cable driver currently supports the Baofeng UV-5R only. Use the
              Export button for other radios and program them in CHIRP.
            </p>
          </div>
        ) : (
          <>
            {/* Safety banner */}
            <div className="flex items-start gap-2 border-b border-sky-200 bg-sky-50 px-5 py-2.5 text-xs text-sky-800 dark:border-sky-900/50 dark:bg-sky-950/40 dark:text-sky-300">
              <ShieldCheck size={15} className="mt-px shrink-0" />
              <span>
                Programming <strong>downloads a full backup first</strong>, then
                writes the channels, names, and your profile’s radio settings, and
                reads the channels back to verify. Only the values your profile
                defines change — everything else is written back as it was read.
              </span>
            </div>

            {/* Port picker */}
            <div className="flex items-end gap-2 px-5 py-4">
              <label className="flex-1">
                <span className="mb-1 block text-[11px] font-medium uppercase tracking-wide text-slate-500 dark:text-slate-400">
                  Serial port
                </span>
                <Select
                  className="w-full"
                  value={port}
                  onChange={(e) => setPort(e.target.value)}
                >
                  {ports.length === 0 && <option value="">No ports found</option>}
                  {ports.map((p) => (
                    <option key={p.name} value={p.name}>
                      {p.name}
                      {p.kind === "usb" ? "  ·  USB" : ""}
                      {p.product ? `  ·  ${p.product}` : ""}
                    </option>
                  ))}
                </Select>
              </label>
              <Button variant="ghost" onClick={refreshPorts} title="Rescan ports">
                <RefreshCw size={14} />
              </Button>
            </div>

            {/* Actions */}
            <div className="flex flex-wrap items-center gap-2 px-5 pb-4">
              <Button onClick={doIdentify} disabled={!port || busy !== null}>
                {busy === "identify" ? <Spinner className="h-3.5 w-3.5" /> : <Search size={14} />}
                Identify
              </Button>
              <Button onClick={doDownload} disabled={!port || busy !== null}>
                {busy === "download" ? <Spinner className="h-3.5 w-3.5" /> : <DownloadCloud size={14} />}
                Download backup
              </Button>
              <Button
                onClick={doRestore}
                disabled={!port || busy !== null}
                title="Flash a previously-saved backup .img back to the radio"
              >
                {busy === "restore" ? <Spinner className="h-3.5 w-3.5" /> : <Undo2 size={14} />}
                Restore backup…
              </Button>
              <div className="ml-auto">
                <Button
                  variant="primary"
                  onClick={() => setConfirming(true)}
                  disabled={!port || busy !== null || writeCount === 0}
                  title={writeCount === 0 ? "This codeplug has no channels to program" : undefined}
                >
                  {busy === "program" ? <Spinner className="h-3.5 w-3.5" /> : <Upload size={14} />}
                  Program radio
                </Button>
              </div>
            </div>

            {/* Confirm write */}
            {confirming && (
              <div className="mx-5 mb-4 rounded-md border border-amber-300 bg-amber-50 p-3 text-xs dark:border-amber-900/50 dark:bg-amber-950/40">
                <div className="mb-2 flex items-center gap-1.5 font-semibold text-amber-800 dark:text-amber-300">
                  <AlertTriangle size={14} /> Confirm write to {modelName}
                </div>
                <p className="text-amber-800 dark:text-amber-200">
                  This will write <strong>{writeCount}</strong> channel
                  {writeCount === 1 ? "" : "s"} to slots 1–{writeCount} and{" "}
                  <strong>clear the remaining {clearCount}</strong> slot
                  {clearCount === 1 ? "" : "s"} so the radio matches “{codeplugName}”.
                  A full backup is saved first.
                </p>

                {skipped.length > 0 && (
                  <div className="mt-3 rounded border border-amber-300/70 bg-amber-100/60 p-2 dark:border-amber-800/60 dark:bg-amber-900/30">
                    <div className="mb-1 font-semibold text-amber-900 dark:text-amber-200">
                      {skipped.length} channel{skipped.length === 1 ? "" : "s"} will be
                      skipped (not supported by {modelName}):
                    </div>
                    <div className="max-h-32 overflow-auto">
                      <ul className="space-y-0.5">
                        {skipped.map((r) => (
                          <li
                            key={r.channel_id}
                            className="flex flex-wrap items-baseline gap-x-1.5 text-amber-800 dark:text-amber-200"
                          >
                            <span className="font-medium">{r.name || "—"}</span>
                            <span className="font-mono text-[10px]">
                              {r.rx_freq.toFixed(4)} MHz
                            </span>
                            {r.mode && (
                              <span className="rounded bg-amber-200/70 px-1 text-[10px] font-semibold uppercase dark:bg-amber-800/60">
                                {r.mode}
                              </span>
                            )}
                            {r.reason && (
                              <span className="text-[10px] opacity-80">— {r.reason}</span>
                            )}
                          </li>
                        ))}
                      </ul>
                    </div>
                    <div className="mt-1.5 text-[10px] opacity-80">
                      These are dropped and the remaining channels close up with
                      no gaps.
                    </div>
                  </div>
                )}

                <div className="mt-3 flex justify-end gap-2">
                  <Button variant="ghost" onClick={() => setConfirming(false)}>
                    Cancel
                  </Button>
                  <Button variant="primary" onClick={doProgram}>
                    <Upload size={14} /> Write to radio
                  </Button>
                </div>
              </div>
            )}

            {/* Progress note while programming */}
            {busy === "program" && (
              <div className="mx-5 mb-4 flex items-center gap-2 rounded-md border border-slate-200 bg-slate-50 px-3 py-2 text-xs text-slate-600 dark:border-slate-700 dark:bg-slate-800/50 dark:text-slate-300">
                <Spinner className="h-3.5 w-3.5" />
                Backing up → writing channels → verifying… keep the radio on and
                the cable connected.
              </div>
            )}

            {/* Status / results */}
            <div className="px-5 pb-5">
              {error && (
                <div className="flex items-start gap-2 rounded-md border border-red-200 bg-red-50 px-3 py-2 text-xs text-red-600 dark:border-red-900/50 dark:bg-red-950/40 dark:text-red-300">
                  <XCircle size={14} className="mt-px shrink-0" />
                  <span>{error}</span>
                </div>
              )}

              {ident && !download && !program && (
                <div className="flex items-center gap-2 rounded-md border border-emerald-200 bg-emerald-50 px-3 py-2 text-xs text-emerald-700 dark:border-emerald-900/50 dark:bg-emerald-950/40 dark:text-emerald-300">
                  <CheckCircle2 size={14} className="shrink-0" />
                  <span>
                    Radio responded ({ident.matched_magic}). Ident:{" "}
                    <span className="font-mono">{ident.ident_hex}</span>
                  </span>
                </div>
              )}

              {download && !program && (
                <ResultBlock
                  ok
                  heading={`Read ${download.image_bytes.toLocaleString()} bytes · ${download.channel_count} programmed channel${download.channel_count === 1 ? "" : "s"}`}
                  backupPath={download.backup_path}
                  backupLabel="Backup saved"
                  channels={download.channels}
                />
              )}

              {program && (
                <ResultBlock
                  ok={program.verified === true}
                  heading={
                    (program.verified === true
                      ? `Programmed ${program.channels_written} channel${program.channels_written === 1 ? "" : "s"} · verified ✓`
                      : `Wrote ${program.channels_written} channel${program.channels_written === 1 ? "" : "s"} — verification warning`) +
                    (program.settings_written != null
                      ? ` · ${program.settings_written} setting${program.settings_written === 1 ? "" : "s"}`
                      : "")
                  }
                  note={program.note ?? undefined}
                  backupPath={program.backup_path}
                  backupLabel="Pre-write backup"
                  channels={program.channels}
                />
              )}
            </div>
          </>
        )}

        <div className="flex items-center justify-end gap-2 border-t border-slate-200 px-4 py-3 dark:border-slate-700">
          <Button variant="ghost" onClick={onClose}>
            Close
          </Button>
        </div>
      </div>
    </Modal>
  );
}

function ResultBlock({
  ok,
  heading,
  note,
  backupPath,
  backupLabel,
  channels,
}: {
  ok: boolean;
  heading: string;
  note?: string;
  backupPath: string;
  backupLabel: string;
  channels: { index: number; name: string; rx_mhz: number; tone: string; power: string }[];
}) {
  return (
    <div className="space-y-3">
      <div
        className={
          ok
            ? "flex items-center gap-2 rounded-md border border-emerald-200 bg-emerald-50 px-3 py-2 text-xs text-emerald-700 dark:border-emerald-900/50 dark:bg-emerald-950/40 dark:text-emerald-300"
            : "flex items-start gap-2 rounded-md border border-amber-300 bg-amber-50 px-3 py-2 text-xs text-amber-800 dark:border-amber-900/50 dark:bg-amber-950/40 dark:text-amber-200"
        }
      >
        {ok ? (
          <CheckCircle2 size={14} className="shrink-0" />
        ) : (
          <AlertTriangle size={14} className="mt-px shrink-0" />
        )}
        <span>{heading}</span>
      </div>

      {note && (
        <p className="text-[11px] text-amber-700 dark:text-amber-300">{note}</p>
      )}

      <div className="text-[11px] text-slate-500 dark:text-slate-400">
        {backupLabel}:{" "}
        <span className="font-mono break-all text-slate-700 dark:text-slate-200">
          {backupPath}
        </span>
      </div>

      {channels.length > 0 && (
        <div>
          <div className="mb-1 flex items-center gap-1.5 text-[11px] font-medium uppercase tracking-wide text-slate-500 dark:text-slate-400">
            <Usb size={12} /> Channels on the radio
          </div>
          <div className="max-h-48 overflow-auto rounded-md border border-slate-200 dark:border-slate-700">
            <table className="w-full text-left text-[11px]">
              <thead className="sticky top-0 bg-slate-100 text-[10px] uppercase tracking-wide text-slate-500 dark:bg-slate-900 dark:text-slate-400">
                <tr>
                  <th className="px-3 py-1.5 font-semibold">#</th>
                  <th className="px-3 py-1.5 font-semibold">Name</th>
                  <th className="px-3 py-1.5 font-semibold">RX (MHz)</th>
                  <th className="px-3 py-1.5 font-semibold">Tone</th>
                  <th className="px-3 py-1.5 font-semibold">Power</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-slate-100 dark:divide-slate-800">
                {channels.map((c) => (
                  <tr key={c.index} className="text-slate-700 dark:text-slate-200">
                    <td className="px-3 py-1 font-mono">{c.index}</td>
                    <td className="px-3 py-1">{c.name || "—"}</td>
                    <td className="px-3 py-1 font-mono">{c.rx_mhz.toFixed(4)}</td>
                    <td className="px-3 py-1">{c.tone}</td>
                    <td className="px-3 py-1">{c.power}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      )}
    </div>
  );
}
