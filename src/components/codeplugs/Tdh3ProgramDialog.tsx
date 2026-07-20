import { useEffect, useState } from "react";
import {
  RefreshCw,
  Search,
  DownloadCloud,
  Upload,
  CheckCircle2,
  XCircle,
  ShieldCheck,
  AlertTriangle,
  Usb,
} from "lucide-react";
import { api } from "../../lib/api";
import type {
  ExportPreview,
  PortInfo,
  DecodedChannelSample,
  DownloadResult,
  RadioIdent,
  Tdh3DecodedChannel,
  Tdh3ProgramResult,
} from "../../lib/types";
import { Modal } from "../overlays";
import { Button, Spinner, Select } from "../ui";
import { Tdh3RadioOptions } from "./Tdh3RadioOptions";

// TD-H3 holds 199 programmable channels (memory_bounds 1..199).
const TDH3_CAPACITY = 199;

/**
 * Direct TIDRADIO TD-H3 programming over the radio's built-in USB-C port.
 *
 * Read: Identify confirms the connection; Download saves a full backup + decoded
 * sample. Write: Program downloads + backs up the radio first, patches only the
 * channel/name regions and the used/scan bitmaps into that image, uploads the
 * whole main range (so all other settings are preserved), and reads back to
 * verify. The radio's non-channel settings are written back untouched.
 */
/// See the note in `ProgramRadioDialog` — per-dialog until 3.7.
const DRIVER_KEY = "tidradio_tdh3";

export function Tdh3ProgramDialog({
  open,
  onClose,
  codeplugId,
  codeplugName,
  modelName,
  modelId,
}: {
  open: boolean;
  onClose: () => void;
  codeplugId: number;
  codeplugName: string;
  modelName: string;
  modelId: number;
}) {
  const [ports, setPorts] = useState<PortInfo[]>([]);
  const [port, setPort] = useState<string>("");
  const [preview, setPreview] = useState<ExportPreview | null>(null);
  const [busy, setBusy] = useState<null | "identify" | "download" | "program">(null);
  const [ident, setIdent] = useState<RadioIdent | null>(null);
  const [download, setDownload] = useState<DownloadResult | null>(null);
  const [program, setProgram] = useState<Tdh3ProgramResult | null>(null);
  const [confirming, setConfirming] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [tab, setTab] = useState<"channels" | "options">("channels");

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
    setTab("channels");
    refreshPorts();
    api.exportPreview(codeplugId).then(setPreview).catch(() => setPreview(null));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  const run = async (
    kind: "identify" | "download" | "program",
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
      setProgram(await api.programTdh3Codeplug(codeplugId, port));
    });

  const writeCount = preview?.included_count ?? 0;
  const clearCount = Math.max(0, TDH3_CAPACITY - writeCount);
  const skipped = (preview?.rows ?? []).filter((r) => !r.included);

  return (
    <Modal
      open={open}
      onClose={onClose}
      title={`Program Radio — ${codeplugName}`}
      width="max-w-2xl"
    >
      <div className="flex flex-col">
        {/* Banner */}
        <div className="flex items-start gap-2 border-b border-sky-200 bg-sky-50 px-5 py-2.5 text-xs text-sky-800 dark:border-sky-900/50 dark:bg-sky-950/40 dark:text-sky-300">
          <ShieldCheck size={15} className="mt-px shrink-0" />
          <span>
            Power the {modelName} on and connect it with a{" "}
            <strong>USB-A&nbsp;→&nbsp;USB-C cable</strong> (C-to-C does not
            enumerate). Programming <strong>downloads a full backup first</strong>,
            then writes the channels and names; every other setting on the radio
            is read and written back unchanged. Tip: if Download/Program says
            “no response” right after Identify, power-cycle the radio first.
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

        {/* Tabs */}
        <div className="flex gap-1 border-b border-slate-200 px-5 dark:border-slate-700">
          {(["channels", "options"] as const).map((t) => (
            <button
              key={t}
              type="button"
              onClick={() => setTab(t)}
              className={
                tab === t
                  ? "-mb-px border-b-2 border-sky-600 px-3 py-2 text-xs font-semibold text-sky-700 dark:border-sky-400 dark:text-sky-300"
                  : "-mb-px border-b-2 border-transparent px-3 py-2 text-xs font-medium text-slate-500 hover:text-slate-700 dark:text-slate-400 dark:hover:text-slate-200"
              }
            >
              {t === "channels" ? "Channels" : "Radio Options"}
            </button>
          ))}
        </div>

        {tab === "options" ? (
          <Tdh3RadioOptions port={port} modelName={modelName} modelId={modelId} />
        ) : (
        <>
        {/* Actions */}
        <div className="flex flex-wrap items-center gap-2 px-5 pb-4 pt-4">
          <Button onClick={doIdentify} disabled={!port || busy !== null}>
            {busy === "identify" ? <Spinner className="h-3.5 w-3.5" /> : <Search size={14} />}
            Identify
          </Button>
          <Button onClick={doDownload} disabled={!port || busy !== null}>
            {busy === "download" ? <Spinner className="h-3.5 w-3.5" /> : <DownloadCloud size={14} />}
            Download backup
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
                        {r.reason && (
                          <span className="text-[10px] opacity-80">— {r.reason}</span>
                        )}
                      </li>
                    ))}
                  </ul>
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
            Backing up → writing channels → verifying… keep the radio on and the
            cable connected.
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
                Radio responded. Ident:{" "}
                <span className="font-mono">{ident.ident_hex}</span>
                {ident.ident_ascii?.trim() && (
                  <>
                    {" "}
                    (<span className="font-mono">{ident.ident_ascii}</span>)
                  </>
                )}
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
              footer="Eyeball these against the radio. If the names, frequencies, and tones match, the data mapping is correct."
            />
          )}

          {program && (
            <ResultBlock
              ok={program.verified}
              heading={
                program.verified
                  ? `Programmed ${program.written} channel${program.written === 1 ? "" : "s"} · verified ✓`
                  : `Wrote ${program.written} channel${program.written === 1 ? "" : "s"} — verification warning`
              }
              note={program.verify_note ?? undefined}
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
  footer,
}: {
  ok: boolean;
  heading: string;
  note?: string;
  backupPath: string;
  backupLabel: string;
  // Both the generic download sample and the TD-H3 program result feed this;
  // the sample nulls `shift`/`mode` on radios that don't decode them.
  channels: (Tdh3DecodedChannel | DecodedChannelSample)[];
  footer?: string;
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

      {note && <p className="text-[11px] text-amber-700 dark:text-amber-300">{note}</p>}

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
                  <th className="px-3 py-1.5 font-semibold">Shift</th>
                  <th className="px-3 py-1.5 font-semibold">Tone</th>
                  <th className="px-3 py-1.5 font-semibold">Mode</th>
                  <th className="px-3 py-1.5 font-semibold">Power</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-slate-100 dark:divide-slate-800">
                {channels.map((c) => (
                  <tr key={c.index} className="text-slate-700 dark:text-slate-200">
                    <td className="px-3 py-1 font-mono">{c.index}</td>
                    <td className="px-3 py-1">{c.name || "—"}</td>
                    <td className="px-3 py-1 font-mono">{c.rx_mhz.toFixed(4)}</td>
                    <td className="px-3 py-1 font-mono">{c.shift || "—"}</td>
                    <td className="px-3 py-1">{c.tone}</td>
                    <td className="px-3 py-1">{c.mode || "—"}</td>
                    <td className="px-3 py-1">{c.power}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          {footer && (
            <p className="mt-2 text-[11px] text-slate-500 dark:text-slate-400">{footer}</p>
          )}
        </div>
      )}
    </div>
  );
}
