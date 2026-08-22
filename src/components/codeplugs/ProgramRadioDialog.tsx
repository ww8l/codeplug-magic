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
  MemoryStick,
  Ear,
} from "lucide-react";
import { api } from "../../lib/api";
import type {
  DownloadResult,
  ExportPreview,
  MemoryCard,
  PortInfo,
  CodeplugProgramReport,
  RadioIdent,
} from "../../lib/types";
import { Modal } from "../overlays";
import { WarningList } from "./ReportWarnings";
import { Button, Spinner, Select } from "../ui";
import {
  driverKeyOf,
  isProgrammable,
  mediaWriteFor,
  useDriverCapabilities,
  type ProgramDialogProps,
} from "../../lib/radioProgramming";

/**
 * The generic, capability-driven program dialog — the default any radio gets
 * (Chunk 3.7), and what the UV-5R uses.
 *
 * Read: identify + download a full backup image. Write: `program_radio`
 * always downloads + backs up the radio first, then writes only the channel +
 * name regions and reads them back to verify. Settings/aux memory are
 * round-tripped untouched.
 *
 * Some radios are programmed by writing a file onto their own memory card
 * rather than over a cable (`mediaWriteFor`). That is still programming, so it
 * appears here as a "Write to SD card…" action; a radio with no cable path at
 * all simply gets no port picker and no cable actions.
 *
 * It names no radio: the driver comes from `model.driver_key`, the offered
 * actions from that driver's capability flags, and the media path from the
 * `export_format` map, so registering a new driver is enough to make this
 * dialog work for it.
 */
export function ProgramRadioDialog({
  open,
  onClose,
  codeplugId,
  codeplugName,
  model,
}: ProgramDialogProps) {
  const driverKey = driverKeyOf(model);
  const caps = useDriverCapabilities(driverKey || null);
  const modelName = model?.display_name ?? "this radio";
  // How this radio takes a codeplug from removable media, if it does.
  const media = mediaWriteFor(model);
  // Programmable at all? Export-only models (no driver) get the "not
  // supported" message instead of buttons that would fail at the port.
  const isSupported = isProgrammable(model) || media != null;
  // Anything that actually talks over the port. `program_codeplug` writes from
  // the database, `program_image` clones whole images, `write_channels` is the
  // channel-only path; a driver with none of them has no cable path at all.
  const cableCapable =
    caps != null &&
    (caps.program_image || caps.program_codeplug || caps.write_channels);
  // Only media radios can lack a cable path, so gate on `media` first: that
  // way the cable UI never flashes on for a radio that will hide it once the
  // capabilities land, and a normal radio never waits on them.
  const showCable = isProgrammable(model) && (media == null || cableCapable);
  const [ports, setPorts] = useState<PortInfo[]>([]);
  const [port, setPort] = useState<string>("");
  const [preview, setPreview] = useState<ExportPreview | null>(null);
  const [busy, setBusy] = useState<
    null | "identify" | "download" | "program" | "restore" | "media"
  >(null);
  const [ident, setIdent] = useState<RadioIdent | null>(null);
  const [download, setDownload] = useState<DownloadResult | null>(null);
  const [program, setProgram] = useState<CodeplugProgramReport | null>(null);
  const [confirming, setConfirming] = useState(false);
  const [mediaWritten, setMediaWritten] = useState<{
    count: number;
    path: string;
  } | null>(null);
  const [cards, setCards] = useState<MemoryCard[] | null>(null);
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
    setMediaWritten(null);
    setError(null);
    setBusy(null);
    setCards(null);
    refreshPorts();
    if (media) {
      api
        .findMemoryCards(model?.export_format ?? "")
        .then(setCards)
        .catch(() => setCards([]));
    }
    if (isSupported) {
      api.exportPreview(codeplugId).then(setPreview).catch(() => setPreview(null));
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  const run = async (
    kind: "identify" | "download" | "program" | "restore" | "media",
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
      setIdent(await api.identifyRadio(driverKey, port));
    });

  const doDownload = () =>
    run("download", async () => {
      setProgram(null);
      setDownload(await api.downloadImage(driverKey, port));
    });

  const doProgram = () =>
    run("program", async () => {
      setConfirming(false);
      setDownload(null);
      setIdent(null);
      setProgram(await api.programRadio(codeplugId, port));
    });

  /// Patch the codeplug into the file the radio reads off its own memory card.
  /// The picker runs before `run` so cancelling it is not an error and leaves
  /// no spinner behind.
  const doMediaWrite = async (target?: string) => {
    if (!media) return;
    let path = target;
    if (!path) {
      const { open: pick } = await import("@tauri-apps/plugin-dialog");
      const picked = await pick({
        title: media.pickTitle,
        multiple: false,
        directory: false,
        // Open ON the card when one was found, so picking a file by hand still
        // does not mean navigating to it.
        defaultPath: cards?.[0]?.path,
        filters: [{ name: media.filterName, extensions: media.extensions }],
      });
      if (typeof picked !== "string") return;
      path = picked;
    }
    const to = path;
    await run("media", async () => {
      setMediaWritten(null);
      // Report the path the exporter WROTE, not the one we asked it to write:
      // handed a card folder it creates the file and names it, and that name is
      // what the operator has to find on the radio's own Load screen.
      const written = await api.generateCodeplug(codeplugId, to);
      setMediaWritten({ count: written.channels, path: written.path });
      // Settings left out of the file because they are outside the range the
      // schema declares — the card was still written (#87).
      if (written.note) {
        const { toast } = await import("sonner");
        toast.warning(written.note, { duration: 12000 });
      }
    });
  };

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
  // Programmed, but the radio cannot transmit there — a memory the operator can
  // only listen on. Worth saying out loud before the write, not after.
  const receiveOnly = (preview?.rows ?? []).filter(
    (r) => r.included && r.receive_only,
  );

  return (
    <Modal
      open={open}
      onClose={onClose}
      title={`Program Radio — ${codeplugName}`}
      width="max-w-2xl"
      // A radio operation is in flight. The Tauri command runs to completion
      // whatever this dialog does, so dismissing it would leave the radio being
      // written while the operator looks at an ordinary page — no spinner, no
      // "keep the radio on", no backup path. (#65)
      dismissible={busy === null}
      lockedHint="Closing now would hide the write, not stop it — the radio is still being written to."
    >
      <div className="flex flex-col">
        {!isSupported ? (
          <div className="px-5 py-10 text-center">
            <RadioTower size={32} className="mx-auto mb-2 text-slate-400" />
            <p className="text-sm font-medium text-slate-700 dark:text-slate-200">
              Direct programming isn’t available for {modelName}.
            </p>
            <p className="mt-1 text-xs text-slate-500 dark:text-slate-400">
              No cable driver is compiled in for this model. Use the Export
              button and program it in CHIRP instead.
            </p>
          </div>
        ) : (
          <>
            {/* Safety banner */}
            {showCable && (
              <div className="flex items-start gap-2 border-b border-sky-200 bg-sky-50 px-5 py-2.5 text-xs text-sky-800 dark:border-sky-900/50 dark:bg-sky-950/40 dark:text-sky-300">
                <ShieldCheck size={15} className="mt-px shrink-0" />
                <span>
                  Programming <strong>downloads a full backup first</strong>, then
                  writes the channels, names, and your profile’s radio settings, and
                  reads the channels back to verify. Only the values your profile
                  defines change — everything else is written back as it was read.
                </span>
              </div>
            )}

            {media && (
              <div className="flex items-start gap-2 border-b border-amber-200 bg-amber-50 px-5 py-2.5 text-xs text-amber-900 dark:border-amber-900/50 dark:bg-amber-950/40 dark:text-amber-200">
                <MemoryStick size={15} className="mt-px shrink-0" />
                <span>
                  {cards?.length === 1 ? (
                    <>
                      <strong>
                        Found the radio’s card mounted as “{cards[0].volume}”.
                      </strong>{" "}
                      {media.found}{" "}
                    </>
                  ) : (
                    <strong>{media.before} </strong>
                  )}
                  {media.preserves}
                </span>
              </div>
            )}

            {/* Port picker */}
            {showCable && (
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
            )}

            {/* What this codeplug will actually do to the radio. Shown before
                the buttons, not behind a confirmation step: the SD-card path
                writes as soon as you click, so a "3 DMR channels are being
                dropped" warning that only appears in the cable confirmation
                never reaches half the radios. */}
            {preview && (
              <div className={`space-y-2 px-5 ${showCable ? "" : "pt-4"}`}>
                <div className="text-xs text-slate-600 dark:text-slate-300">
                  Programming <strong>{writeCount}</strong> channel
                  {writeCount === 1 ? "" : "s"} into {modelName}
                  {receiveOnly.length > 0 && (
                    <>
                      {" "}
                      · <strong>{receiveOnly.length}</strong> receive-only
                    </>
                  )}
                  {skipped.length > 0 && (
                    <>
                      {" "}
                      · <strong>{skipped.length}</strong> skipped
                    </>
                  )}
                </div>

                {receiveOnly.length > 0 && (
                  <ChannelNotice
                    tone="sky"
                    title={`${receiveOnly.length} channel${receiveOnly.length === 1 ? " is" : "s are"} receive-only on ${modelName}`}
                    rows={receiveOnly}
                    footer={`The radio hears these but its transmitter does not reach them, so a stock ${modelName} will refuse to key up — they are programmed anyway, and a radio whose transmit range has been opened up will use them exactly as written.`}
                  />
                )}

                {skipped.length > 0 && (
                  <ChannelNotice
                    tone="amber"
                    title={`${skipped.length} channel${skipped.length === 1 ? "" : "s"} will be skipped — ${modelName} cannot use ${skipped.length === 1 ? "it" : "them"}`}
                    rows={skipped}
                    footer="These are dropped and the remaining channels close up with no gaps."
                  />
                )}
              </div>
            )}

            {/* Actions */}
            <div
              className={`flex flex-wrap items-center gap-2 px-5 pb-4 ${showCable && !preview ? "" : "pt-4"}`}
            >
              {showCable && (
              <Button onClick={doIdentify} disabled={!port || busy !== null}>
                {busy === "identify" ? <Spinner className="h-3.5 w-3.5" /> : <Search size={14} />}
                Identify
              </Button>
              )}
              {/* Backup/restore are whole-image operations, so they exist only
                  on clone radios. A driver that programs from the database
                  (the AnyTone) takes its own backups inside the program run. */}
              {showCable && caps?.program_image && (
                <>
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
                </>
              )}
              <div className="ml-auto flex flex-wrap items-center gap-2">
                {media && (
                  <>
                    {/* One detected card is the whole point: write to it
                        directly. Browse stays available for the awkward cases
                        (card copied to disk, several radios, no card yet). */}
                    {cards?.length === 1 && (
                      <Button
                        variant="primary"
                        onClick={() => doMediaWrite(cards[0].path)}
                        disabled={busy !== null || writeCount === 0}
                        title={cards[0].path}
                      >
                        {busy === "media" ? (
                          <Spinner className="h-3.5 w-3.5" />
                        ) : (
                          <MemoryStick size={14} />
                        )}
                        Write to “{cards[0].volume}”
                      </Button>
                    )}
                    <Button
                      variant={
                        showCable || cards?.length === 1 ? undefined : "primary"
                      }
                      onClick={() => doMediaWrite()}
                      disabled={busy !== null || writeCount === 0}
                      title={
                        writeCount === 0
                          ? "This codeplug has no channels to program"
                          : "Patch this codeplug into the backup file on the radio’s memory card"
                      }
                    >
                      {busy === "media" && cards?.length !== 1 ? (
                        <Spinner className="h-3.5 w-3.5" />
                      ) : (
                        <MemoryStick size={14} />
                      )}
                      {cards?.length === 1 ? "Choose a file…" : media.action}
                    </Button>
                  </>
                )}
                {showCable && (
                  <Button
                    variant="primary"
                    onClick={() => setConfirming(true)}
                    disabled={!port || busy !== null || writeCount === 0}
                    title={writeCount === 0 ? "This codeplug has no channels to program" : undefined}
                  >
                    {busy === "program" ? <Spinner className="h-3.5 w-3.5" /> : <Upload size={14} />}
                    Program radio
                  </Button>
                )}
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

                {(skipped.length > 0 || receiveOnly.length > 0) && (
                  <p className="mt-2 text-amber-800 dark:text-amber-200">
                    {[
                      skipped.length > 0 &&
                        `${skipped.length} channel${skipped.length === 1 ? " is" : "s are"} being skipped`,
                      receiveOnly.length > 0 &&
                        `${receiveOnly.length} ${receiveOnly.length === 1 ? "is" : "are"} receive-only`,
                    ]
                      .filter(Boolean)
                      .join(" and ")}{" "}
                    — listed above.
                  </p>
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

              {mediaWritten && media && (
                <div className="space-y-1.5 rounded-md border border-emerald-200 bg-emerald-50 px-3 py-2 text-xs text-emerald-700 dark:border-emerald-900/50 dark:bg-emerald-950/40 dark:text-emerald-300">
                  <div className="flex items-center gap-2">
                    <CheckCircle2 size={14} className="shrink-0" />
                    <span>
                      Wrote {mediaWritten.count} channel
                      {mediaWritten.count === 1 ? "" : "s"} and your profile’s
                      settings into{" "}
                      <span className="font-mono">
                        {mediaWritten.path.split("/").pop()}
                      </span>
                      .
                    </span>
                  </div>
                  <div className="pl-6 text-emerald-800 dark:text-emerald-200">
                    {media.after}
                  </div>
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
                  warnings={program.warnings}
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

/// A labelled list of channels the write is going to treat specially, with the
/// per-channel reason the backend gave. Amber warns (dropped); sky informs
/// (programmed, but listen-only).
function ChannelNotice({
  tone,
  title,
  rows,
  footer,
}: {
  tone: "amber" | "sky";
  title: string;
  rows: ExportPreview["rows"];
  footer: string;
}) {
  const palette =
    tone === "amber"
      ? {
          box: "border-amber-300/70 bg-amber-50 dark:border-amber-800/60 dark:bg-amber-950/30",
          head: "text-amber-900 dark:text-amber-200",
          body: "text-amber-800 dark:text-amber-200",
          chip: "bg-amber-200/70 dark:bg-amber-800/60",
        }
      : {
          box: "border-sky-300/70 bg-sky-50 dark:border-sky-800/60 dark:bg-sky-950/30",
          head: "text-sky-900 dark:text-sky-200",
          body: "text-sky-800 dark:text-sky-200",
          chip: "bg-sky-200/70 dark:bg-sky-800/60",
        };
  return (
    <div className={`rounded border p-2 text-xs ${palette.box}`}>
      <div className={`mb-1 flex items-center gap-1.5 font-semibold ${palette.head}`}>
        {tone === "amber" ? <AlertTriangle size={13} /> : <Ear size={13} />}
        {title}
      </div>
      <div className="max-h-32 overflow-auto">
        <ul className="space-y-0.5">
          {rows.map((r) => (
            <li
              key={r.channel_id}
              className={`flex flex-wrap items-baseline gap-x-1.5 ${palette.body}`}
            >
              <span className="font-medium">{r.name || "—"}</span>
              <span className="font-mono text-[10px]">
                {r.rx_freq.toFixed(4)} MHz
              </span>
              {r.mode && (
                <span
                  className={`rounded px-1 text-[10px] font-semibold uppercase ${palette.chip}`}
                >
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
      <div className={`mt-1.5 text-[10px] opacity-80 ${palette.body}`}>{footer}</div>
    </div>
  );
}

function ResultBlock({
  ok,
  heading,
  note,
  warnings,
  backupPath,
  backupLabel,
  channels,
}: {
  ok: boolean;
  heading: string;
  note?: string;
  /// Things the write did differently from what was asked — a settings value
  /// outside its schema's range and left behind, for one. The command layer
  /// fills these for every driver; only the AnyTone dialog was reading them.
  warnings?: string[];
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

      {warnings && warnings.length > 0 && <WarningList warnings={warnings} />}

      <div className="text-[11px] text-slate-500 dark:text-slate-400">
        {backupLabel}:{" "}
        <span className="font-mono break-all text-slate-700 dark:text-slate-200">
          {backupPath}
        </span>
      </div>

      {channels.length > 0 && (
        <div>
          <div className="mb-1 flex items-center gap-1.5 text-[11px] font-medium uppercase tracking-wide text-slate-500 dark:text-slate-400">
            <Usb size={12} /> Channels on the radio ({channels.length})
          </div>
          <div className="max-h-72 overflow-auto rounded-md border border-slate-200 dark:border-slate-700">
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
