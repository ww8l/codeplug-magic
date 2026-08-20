import { useEffect, useMemo, useState } from "react";
import {
  RefreshCw,
  Upload,
  CheckCircle2,
  XCircle,
  ShieldCheck,
  AlertTriangle,
  ListChecks,
  Users,
  Settings2,
  Undo2,
  ArrowUp,
  ArrowDown,
  X,
} from "lucide-react";
import { confirm as confirmDialog } from "@tauri-apps/plugin-dialog";
import { api } from "../../lib/api";
import type {
  AnytoneProgramPreview,
  CodeplugProgramReport,
  SettingsWriteReport,
  CallsignDbReport,
  AnytoneLatestProgram,
  AnytonePatchWriteResult,
  AnytoneVerifyResult,
  DmrExportPreview,
  PortInfo,
} from "../../lib/types";
import { Modal } from "../overlays";
import { Button, Spinner, Select, TextInput } from "../ui";
import type { ProgramDialogProps } from "../../lib/radioProgramming";

type Payload = "channels" | "userdb" | "profile";

type Busy = null | "program" | "verify" | "restore";

/// Last path segment, for naming a backup in a confirmation. The full path is
/// an app-data directory nobody reads; the file name carries the timestamp,
/// which is the part that identifies WHICH backup.
function fileName(path: string): string {
  return path.split(/[\\/]/).pop() || path;
}

/**
 * Direct AnyTone AT-D890UV programming from the database: a FULL REPLACE of
 * the radio's channel set (channels + zones + scan lists + DMR contacts) in
 * one PC-mode session. The radio commits on END and reboots (USB
 * re-enumerates), so Verify runs as a separate step after a rescan.
 *
 * Three payloads are presented: the channel set (full replace), the Profile
 * (radio menu settings, read-modify-write), and UserDB (the DMR caller-ID /
 * "Call-sign Database" pulled from the local DMR Contacts library, prioritized
 * and capacity-capped). UserDB verification is functional (the radio's own
 * caller-ID screen), so only Restore-from-backup is offered for it.
 */
export function AnytoneProgramDialog({
  open,
  onClose,
  codeplugId,
  profileId,
  codeplugName,
  model,
}: ProgramDialogProps) {
  const modelName = model?.display_name ?? "this radio";
  const [ports, setPorts] = useState<PortInfo[]>([]);
  const [port, setPort] = useState<string>("");
  const [payload, setPayload] = useState<Payload>("channels");
  const [preview, setPreview] = useState<AnytoneProgramPreview | null>(null);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const [ack, setAck] = useState(false);
  const [busy, setBusy] = useState<Busy>(null);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<CodeplugProgramReport | null>(null);
  const [settingsResult, setSettingsResult] = useState<SettingsWriteReport | null>(null);
  const [latest, setLatest] = useState<AnytoneLatestProgram | null>(null);
  const [verify, setVerify] = useState<AnytoneVerifyResult | null>(null);
  const [restored, setRestored] = useState<AnytonePatchWriteResult | null>(null);
  const [callsignResult, setCallsignResult] = useState<CallsignDbReport | null>(null);

  // UserDB (Call-sign Database) batch selection — mirrors the DMR-export dialog.
  const [dbCountries, setDbCountries] = useState<string[]>([]);
  const [dbContinents, setDbContinents] = useState<string[]>([]);
  const [dbPriority, setDbPriority] = useState<string[]>([]);
  const [dbAddChoice, setDbAddChoice] = useState("");
  const [dbMaxText, setDbMaxText] = useState("");
  const [dbPreview, setDbPreview] = useState<DmrExportPreview | null>(null);
  const [dbPreviewing, setDbPreviewing] = useState(false);

  const dbMaxCount = useMemo(() => {
    const n = parseInt(dbMaxText, 10);
    return Number.isFinite(n) && n > 0 ? n : null;
  }, [dbMaxText]);

  // Verify/Restore target the in-memory result when we have it, else the newest
  // image on disk — so they stay reachable even when a program's result never
  // came back (the commit reboot drops USB and can break the IPC round-trip).
  const expectedPath =
    result?.expected_path ?? settingsResult?.expected_path ?? latest?.expected_path ?? null;
  const backupPath =
    result?.backup_path ?? settingsResult?.backup_path ?? latest?.backup_path ?? null;
  const usingLatest = !result && !settingsResult && !callsignResult && latest != null;
  // Which write's pre-write backup Verify and Restore are pointed at. Only one
  // of these can be set at a time — `doProgram` clears the others (#63) — so
  // this names the real target rather than guessing at the ?? chain's winner.
  const backupSource = result
    ? "the channel-set program"
    : settingsResult
      ? "the settings write"
      : latest
        ? `the last program on disk (${latest.stamp})`
        : null;

  const refreshPorts = async () => {
    try {
      const list = await api.listSerialPorts();
      setPorts(list);
      const usb = list.find((p) => p.kind === "usb");
      setPort((cur) => (cur && list.some((p) => p.name === cur) ? cur : usb?.name || list[0]?.name || ""));
    } catch (e) {
      setError(typeof e === "string" ? e : String(e));
    }
  };

  useEffect(() => {
    if (!open) return;
    setPayload("channels");
    setPreview(null);
    setPreviewError(null);
    setAck(false);
    setBusy(null);
    setError(null);
    setResult(null);
    setSettingsResult(null);
    setLatest(null);
    setVerify(null);
    setRestored(null);
    setCallsignResult(null);
    setDbPriority([]);
    setDbAddChoice("");
    setDbMaxText("");
    setDbPreview(null);
    refreshPorts();
    api.latestAnytoneProgram().then(setLatest).catch(() => {});
    api.listDmrUserCountries().then(setDbCountries).catch(() => {});
    api.listDmrContinents().then(setDbContinents).catch(() => {});
    api
      .programAnytonePreview(codeplugId)
      .then(setPreview)
      .catch((e) => setPreviewError(typeof e === "string" ? e : String(e)));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  // Debounced Call-sign DB preview (how many contacts, by country) whenever the
  // UserDB payload is active and its selection changes.
  useEffect(() => {
    if (!open || payload !== "userdb") return;
    setDbPreviewing(true);
    const t = setTimeout(() => {
      api
        .previewDmrExport(dbMaxCount, dbPriority)
        .then(setDbPreview)
        .catch(() => setDbPreview(null))
        .finally(() => setDbPreviewing(false));
    }, 200);
    return () => clearTimeout(t);
  }, [open, payload, dbMaxCount, dbPriority]);

  const dbAvailableToAdd = useMemo(() => {
    const chosen = new Set(dbPriority.map((p) => p.toLowerCase()));
    return {
      continents: dbContinents.filter((c) => !chosen.has(c.toLowerCase())),
      countries: dbCountries.filter((c) => !chosen.has(c.toLowerCase())),
    };
  }, [dbContinents, dbCountries, dbPriority]);

  const addDbPriority = () => {
    if (!dbAddChoice) return;
    setDbPriority((p) => [...p, dbAddChoice]);
    setDbAddChoice("");
  };
  const removeDbPriority = (i: number) =>
    setDbPriority((p) => p.filter((_, idx) => idx !== i));
  const moveDbPriority = (i: number, dir: -1 | 1) =>
    setDbPriority((p) => {
      const j = i + dir;
      if (j < 0 || j >= p.length) return p;
      const next = [...p];
      [next[i], next[j]] = [next[j], next[i]];
      return next;
    });

  const run = async (kind: Exclude<Busy, null>, fn: () => Promise<void>) => {
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

  const doProgram = async () => {
    setError(null);
    setBusy("program");
    setVerify(null);
    setRestored(null);
    setAck(false);
    // Clear EVERY payload's previous result, not just this one's. Verify and
    // Restore resolve their target as `result ?? settingsResult ?? latest`, so a
    // leftover result from an earlier payload in the same dialog session wins
    // the chain and Restore hands the radio the wrong backup — a settings write
    // followed by Restore would replay the channel program's pre-write image and
    // wipe the channel set. (#63)
    setResult(null);
    setSettingsResult(null);
    setCallsignResult(null);
    try {
      if (payload === "profile" || payload === "userdb") {
        if (profileId == null) {
          throw new Error(
            "This codeplug has no radio profile — attach one under Codeplugs first.",
          );
        }
        if (payload === "profile") {
          setSettingsResult(await api.writeRadioSettings(port, profileId));
        } else {
          setCallsignResult(
            await api.writeCallsignDb(profileId, port, dbMaxCount, dbPriority),
          );
        }
      } else {
        setResult(await api.programRadio(codeplugId, port));
      }
    } catch (e) {
      // The write may well have committed — the radio reboots and drops USB on
      // the END, which can break the reply before the result reaches us. The
      // backup + expected image are written to disk BEFORE any write, so fall
      // back to them (fetched below) rather than treating this as a dead end.
      const msg = typeof e === "string" ? e : String(e);
      setError(
        `${msg} — the radio may still have programmed; it reboots and drops USB on ` +
          `commit. Rescan, reselect the port, then Verify against the last image below.`,
      );
    } finally {
      setBusy(null);
      // Refresh the newest on-disk image so Verify/Restore are reachable even
      // if the result above never came back.
      api.latestAnytoneProgram().then(setLatest).catch(() => {});
    }
  };

  const doVerify = () =>
    run("verify", async () => {
      if (!expectedPath) return;
      setVerify(await api.verifyAnytoneProgram(port, expectedPath));
    });

  /// Restore is a FULL write that commits and reboots the radio, and it was the
  /// only radio-write button in the app with no gate at all — Program, the
  /// generic dialog, the TD-H3 dialog and the UV-5R restore each have one.
  ///
  /// A native confirm rather than this dialog's `ack` checkbox: Restore is
  /// reached in a hurry, after something already went wrong, and a checkbox
  /// sitting above the button is exactly what a hurried operator ticks without
  /// reading. It also has to name WHICH file — the whole of #63 was this button
  /// silently pointing at the wrong one. (#64)
  const doRestore = async () => {
    if (!backupPath) return;
    const ok = await confirmDialog(
      `Write ${fileName(backupPath)} back to the ${modelName}?\n\n` +
        `That file is the pre-write backup from ${backupSource ?? "an earlier write"}. ` +
        `Restoring is a full write: every window the backup holds is rewritten, ` +
        `then the radio commits and reboots. Anything programmed since that ` +
        `backup was taken is lost.`,
      {
        title: "Restore backup to radio",
        kind: "warning",
        okLabel: "Restore",
        cancelLabel: "Cancel",
      },
    );
    if (!ok) return;
    await run("restore", async () => {
      setVerify(null);
      setRestored(await api.restoreAnytoneBackup(port, backupPath));
    });
  };

  const payloads: {
    id: Payload;
    icon: typeof ListChecks;
    label: string;
    detail: string;
    disabled: boolean;
  }[] = [
    {
      id: "channels",
      icon: ListChecks,
      label: "Channel set",
      detail: "Channels, zones, scan lists, and talkgroups — full replace.",
      disabled: false,
    },
    {
      id: "userdb",
      icon: Users,
      label: "Call-sign DB",
      detail:
        "DMR caller-ID database from your DMR Contacts library — prioritized, capacity-capped. Includes your talkgroups, which name them on the TX screen.",
      disabled: false,
    },
    {
      id: "profile",
      icon: Settings2,
      label: "Profile",
      detail: "Radio menu settings + DMR ID from the profile — read-modify-write, channels untouched.",
      disabled: false,
    },
  ];

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
      lockedHint={`Closing now would hide the write, not stop it — the ${modelName} is still being written to.`}
    >
      <div className="flex flex-col">
        {/* Banner */}
        <div className="flex items-start gap-2 border-b border-sky-200 bg-sky-50 px-5 py-2.5 text-xs text-sky-800 dark:border-sky-900/50 dark:bg-sky-950/40 dark:text-sky-300">
          <ShieldCheck size={15} className="mt-px shrink-0" />
          <span>
            Power the {modelName} on and connect its USB-C port. Programming is a{" "}
            <strong>full replace</strong> of the selected payload — a backup of every
            region touched is saved first. When the write commits, the radio{" "}
            <strong>reboots and drops off USB</strong>: rescan, reselect the port, then
            run <strong>Verify</strong>.
          </span>
        </div>

        {/* Payload picker */}
        <div className="grid grid-cols-3 gap-2 px-5 pt-4">
          {payloads.map((p) => {
            const Icon = p.icon;
            const selected = payload === p.id;
            return (
              <button
                key={p.id}
                type="button"
                disabled={p.disabled}
                onClick={() => setPayload(p.id)}
                title={p.detail}
                className={
                  p.disabled
                    ? "rounded-md border border-slate-200 bg-slate-50 p-3 text-left opacity-50 dark:border-slate-700 dark:bg-slate-800/40"
                    : selected
                      ? "rounded-md border-2 border-sky-500 bg-sky-50 p-3 text-left dark:border-sky-400 dark:bg-sky-950/40"
                      : "rounded-md border border-slate-200 p-3 text-left hover:border-sky-300 dark:border-slate-700 dark:hover:border-sky-700"
                }
              >
                <div className="mb-1 flex items-center gap-1.5 text-xs font-semibold text-slate-700 dark:text-slate-200">
                  <Icon size={14} className={selected && !p.disabled ? "text-sky-600 dark:text-sky-400" : ""} />
                  {p.label}
                </div>
                <div className="text-[10px] leading-snug text-slate-500 dark:text-slate-400">
                  {p.detail}
                </div>
              </button>
            );
          })}
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

        {/* Preview of what will be written */}
        <div className="px-5 pb-4">
          {payload === "profile" && (
            <div className="rounded-md border border-slate-200 p-3 text-xs text-slate-600 dark:border-slate-700 dark:text-slate-300">
              Writes this profile's saved radio settings (the ones from{" "}
              <strong>Download from radio</strong> in the profile editor) back to the
              {" "}{modelName}. Only bytes that differ are changed — every unmodeled
              byte and the entire channel set are preserved. If the profile has no
              saved settings yet, download and save them first.
            </div>
          )}
          {payload === "userdb" && (
            <div className="flex flex-col gap-3">
              <div className="rounded-md border border-slate-200 p-3 text-xs text-slate-600 dark:border-slate-700 dark:text-slate-300">
                Writes the DMR <strong>caller-ID database</strong> from your local{" "}
                <strong>DMR Contacts</strong> library to the {modelName}, so incoming
                calls show the operator's callsign and name. Choose a size cap and a
                country/continent priority — the highest-priority contacts fill the cap
                first. Refresh the library from the DMR Contacts page if it looks empty.
                <div className="mt-2">
                  Your <strong>talkgroups</strong> are always included on top of that cap
                  — this is what lets the radio show a talkgroup's name, not just its
                  number, on the transmit screen.
                </div>
              </div>

              <label className="flex flex-col gap-1">
                <span className="text-[11px] font-medium uppercase tracking-wide text-slate-500 dark:text-slate-400">
                  Max contacts (blank = no limit)
                </span>
                <TextInput
                  inputMode="numeric"
                  placeholder="e.g. 50000 — large batches take longer to write over USB"
                  value={dbMaxText}
                  onChange={(e) => setDbMaxText(e.target.value.replace(/[^0-9]/g, ""))}
                />
              </label>

              <div className="flex flex-col gap-1.5">
                <span className="text-[11px] font-medium uppercase tracking-wide text-slate-500 dark:text-slate-400">
                  Priority order (matched first; everyone else fills any remaining space)
                </span>
                <div className="flex gap-1.5">
                  <Select
                    className="flex-1"
                    value={dbAddChoice}
                    onChange={(e) => setDbAddChoice(e.target.value)}
                  >
                    <option value="">Add a country or continent…</option>
                    {dbAvailableToAdd.continents.length > 0 && (
                      <optgroup label="Continents">
                        {dbAvailableToAdd.continents.map((c) => (
                          <option key={c} value={c}>
                            {c}
                          </option>
                        ))}
                      </optgroup>
                    )}
                    {dbAvailableToAdd.countries.length > 0 && (
                      <optgroup label="Countries">
                        {dbAvailableToAdd.countries.map((c) => (
                          <option key={c} value={c}>
                            {c}
                          </option>
                        ))}
                      </optgroup>
                    )}
                  </Select>
                  <Button onClick={addDbPriority} disabled={!dbAddChoice}>
                    Add
                  </Button>
                </div>
                {dbPriority.length > 0 && (
                  <ul className="flex flex-col gap-1 rounded-md border border-slate-200 p-1.5 dark:border-slate-700">
                    {dbPriority.map((p, i) => (
                      <li
                        key={p}
                        className="flex items-center justify-between gap-2 rounded bg-slate-50 px-2 py-1 text-xs dark:bg-slate-800"
                      >
                        <span className="text-slate-500 tabular-nums">{i + 1}.</span>
                        <span className="flex-1 text-slate-700 dark:text-slate-200">{p}</span>
                        <div className="flex gap-1">
                          <button
                            className="rounded p-0.5 text-slate-400 hover:bg-slate-200 hover:text-slate-600 disabled:opacity-30 dark:hover:bg-slate-700"
                            onClick={() => moveDbPriority(i, -1)}
                            disabled={i === 0}
                            title="Move up"
                          >
                            <ArrowUp size={12} />
                          </button>
                          <button
                            className="rounded p-0.5 text-slate-400 hover:bg-slate-200 hover:text-slate-600 disabled:opacity-30 dark:hover:bg-slate-700"
                            onClick={() => moveDbPriority(i, 1)}
                            disabled={i === dbPriority.length - 1}
                            title="Move down"
                          >
                            <ArrowDown size={12} />
                          </button>
                          <button
                            className="rounded p-0.5 text-slate-400 hover:bg-slate-200 hover:text-rose-600 dark:hover:bg-slate-700"
                            onClick={() => removeDbPriority(i)}
                            title="Remove"
                          >
                            <X size={12} />
                          </button>
                        </div>
                      </li>
                    ))}
                  </ul>
                )}
              </div>

              <div className="rounded-md border border-slate-200 bg-slate-50 p-3 text-xs dark:border-slate-700 dark:bg-slate-800/60">
                {dbPreviewing || !dbPreview ? (
                  <div className="flex items-center gap-2 text-slate-400">
                    <Spinner className="h-3.5 w-3.5" /> Calculating…
                  </div>
                ) : (
                  <>
                    <div className="font-medium text-slate-700 dark:text-slate-200">
                      {dbPreview.will_export.toLocaleString()} of{" "}
                      {dbPreview.total_available.toLocaleString()} contacts will be written
                    </div>
                    {dbPreview.by_country.length > 0 && (
                      <div className="mt-1.5 flex flex-wrap gap-x-3 gap-y-0.5 text-slate-500">
                        {dbPreview.by_country.slice(0, 8).map((b) => (
                          <span key={b.country}>
                            {b.country}: {b.count.toLocaleString()}
                          </span>
                        ))}
                        {dbPreview.by_country.length > 8 && <span>…</span>}
                      </div>
                    )}
                  </>
                )}
              </div>
            </div>
          )}
          {payload === "channels" && previewError && (
            <div className="flex items-start gap-2 rounded-md border border-red-200 bg-red-50 px-3 py-2 text-xs text-red-600 dark:border-red-900/50 dark:bg-red-950/40 dark:text-red-300">
              <XCircle size={14} className="mt-px shrink-0" />
              <span>{previewError}</span>
            </div>
          )}
          {payload === "channels" && !preview && !previewError && (
            <div className="flex items-center gap-2 text-xs text-slate-500 dark:text-slate-400">
              <Spinner className="h-3.5 w-3.5" /> Assembling preview…
            </div>
          )}
          {payload === "channels" && preview && (
            <div className="rounded-md border border-slate-200 p-3 text-xs dark:border-slate-700">
              <div className="mb-2 grid grid-cols-4 gap-2 text-center">
                {(
                  [
                    ["Channels", preview.channels],
                    ["Zones", preview.zones],
                    ["Scan lists", preview.scan_lists],
                    ["Talkgroups", preview.contacts],
                  ] as const
                ).map(([label, n]) => (
                  <div
                    key={label}
                    className="rounded bg-slate-50 py-1.5 dark:bg-slate-800/60"
                  >
                    <div className="text-base font-semibold text-slate-800 dark:text-slate-100">
                      {n}
                    </div>
                    <div className="text-[10px] uppercase tracking-wide text-slate-500 dark:text-slate-400">
                      {label}
                    </div>
                  </div>
                ))}
              </div>
              {preview.zone_names.length > 0 && (
                <div className="text-[11px] text-slate-600 dark:text-slate-300">
                  <span className="font-medium">Zones:</span>{" "}
                  {preview.zone_names.join(" · ")}
                </div>
              )}
              {preview.scan_list_names.length > 0 && (
                <div className="text-[11px] text-slate-600 dark:text-slate-300">
                  <span className="font-medium">Scan lists:</span>{" "}
                  {preview.scan_list_names.join(" · ")}
                </div>
              )}
              {preview.skipped.length > 0 && (
                <div className="mt-2 rounded border border-amber-300/70 bg-amber-50 p-2 dark:border-amber-800/60 dark:bg-amber-950/30">
                  <div className="mb-1 font-semibold text-amber-800 dark:text-amber-300">
                    {preview.skipped.length} channel
                    {preview.skipped.length === 1 ? "" : "s"} will be skipped:
                  </div>
                  <ul className="max-h-24 space-y-0.5 overflow-auto text-amber-800 dark:text-amber-200">
                    {preview.skipped.map((s, i) => (
                      <li key={i}>
                        <span className="font-medium">{s.name || "—"}</span>{" "}
                        <span className="text-[10px] opacity-80">— {s.reason}</span>
                      </li>
                    ))}
                  </ul>
                </div>
              )}
              {preview.warnings.length > 0 && (
                <div className="mt-2 rounded border border-amber-300/70 bg-amber-50 p-2 dark:border-amber-800/60 dark:bg-amber-950/30">
                  <div className="mb-1 flex items-center gap-1 font-semibold text-amber-800 dark:text-amber-300">
                    <AlertTriangle size={12} /> Warnings
                  </div>
                  <ul className="max-h-24 list-inside list-disc space-y-0.5 overflow-auto text-amber-800 dark:text-amber-200">
                    {preview.warnings.map((w, i) => (
                      <li key={i}>{w}</li>
                    ))}
                  </ul>
                </div>
              )}
            </div>
          )}
        </div>

        {/* Ack + program */}
        <div className="flex flex-wrap items-center gap-3 px-5 pb-4">
          <label className="flex items-start gap-2 text-xs text-slate-600 dark:text-slate-300">
            <input
              type="checkbox"
              checked={ack}
              onChange={(e) => setAck(e.target.checked)}
              className="mt-0.5"
            />
            {payload === "profile" ? (
              <span>
                I understand this <strong>overwrites the radio's menu settings</strong>{" "}
                with this profile's saved settings (channels, zones, and contacts are
                left untouched). A backup is saved first.
              </span>
            ) : payload === "userdb" ? (
              <span>
                I understand this <strong>replaces the radio's caller-ID database</strong>{" "}
                (channels, zones, scan lists, and talkgroups are left untouched). It's a
                self-contained, re-writable database — verify it on the radio's caller-ID
                screen after the write.
              </span>
            ) : (
              <span>
                I understand this <strong>replaces the radio's entire channel set</strong>{" "}
                (existing channels, zones, scan lists, and contacts on the radio are
                overwritten or cleared).
              </span>
            )}
          </label>
          <div className="ml-auto">
            <Button
              variant="primary"
              onClick={doProgram}
              disabled={
                !port ||
                busy !== null ||
                !ack ||
                (payload === "channels" && (!preview || preview.channels === 0)) ||
                (payload === "userdb" && (!dbPreview || dbPreview.will_export === 0))
              }
              title={
                payload === "channels" && preview && preview.channels === 0
                  ? "This codeplug has no programmable channels"
                  : payload === "userdb" && dbPreview && dbPreview.will_export === 0
                    ? "No DMR contacts to write — refresh the DMR Contacts library first"
                    : undefined
              }
            >
              {busy === "program" ? <Spinner className="h-3.5 w-3.5" /> : <Upload size={14} />}
              {payload === "profile"
                ? "Write settings"
                : payload === "userdb"
                  ? "Write Call-sign DB"
                  : "Program radio"}
            </Button>
          </div>
        </div>

        {busy === "program" && (
          <div className="mx-5 mb-4 flex items-center gap-2 rounded-md border border-slate-200 bg-slate-50 px-3 py-2 text-xs text-slate-600 dark:border-slate-700 dark:bg-slate-800/50 dark:text-slate-300">
            <Spinner className="h-3.5 w-3.5" />
            Reading regions → backing up → writing → committing… keep the radio on and
            the cable connected. The radio reboots when this finishes.
          </div>
        )}

        {/* Status / results */}
        <div className="space-y-3 px-5 pb-5">
          {error && (
            <div className="flex items-start gap-2 rounded-md border border-red-200 bg-red-50 px-3 py-2 text-xs text-red-600 dark:border-red-900/50 dark:bg-red-950/40 dark:text-red-300">
              <XCircle size={14} className="mt-px shrink-0" />
              <span>{error}</span>
            </div>
          )}

          {result && (
            <div className="space-y-2 rounded-md border border-emerald-200 bg-emerald-50 p-3 text-xs text-emerald-800 dark:border-emerald-900/50 dark:bg-emerald-950/40 dark:text-emerald-200">
              <div className="flex items-center gap-2 font-semibold">
                <CheckCircle2 size={14} className="shrink-0" />
                Wrote {result.channels_written} channels · {result.zones_written} zones ·{" "}
                {result.scan_lists_written} scan lists · {result.contacts_written}{" "}
                talkgroups
                {result.slots_cleared + result.zones_cleared + result.contacts_cleared >
                  0 && (
                  <span className="font-normal">
                    {" "}
                    (cleared {result.slots_cleared} stale slots, {result.zones_cleared}{" "}
                    zones, {result.contacts_cleared} contacts)
                  </span>
                )}
              </div>
              <p>{result.note}</p>
              <div className="text-[11px]">
                Backup:{" "}
                <span className="font-mono break-all">{result.backup_path}</span>
              </div>
            </div>
          )}

          {settingsResult && (
            <div className="space-y-2 rounded-md border border-emerald-200 bg-emerald-50 p-3 text-xs text-emerald-800 dark:border-emerald-900/50 dark:bg-emerald-950/40 dark:text-emerald-200">
              <div className="flex items-center gap-2 font-semibold">
                <CheckCircle2 size={14} className="shrink-0" />
                Wrote settings — {settingsResult.fields_written} byte
                {settingsResult.fields_written === 1 ? "" : "s"} changed across{" "}
                {settingsResult.windows_written.length} window
                {settingsResult.windows_written.length === 1 ? "" : "s"}
              </div>
              <p>{settingsResult.note}</p>
              <div className="text-[11px]">
                Backup:{" "}
                <span className="font-mono break-all">{settingsResult.backup_path}</span>
              </div>
            </div>
          )}

          {callsignResult && (
            <div className="space-y-2 rounded-md border border-emerald-200 bg-emerald-50 p-3 text-xs text-emerald-800 dark:border-emerald-900/50 dark:bg-emerald-950/40 dark:text-emerald-200">
              <div className="flex items-center gap-2 font-semibold">
                <CheckCircle2 size={14} className="shrink-0" />
                Wrote the call-sign database — {callsignResult.entries_written}{" "}
                entr{callsignResult.entries_written === 1 ? "y" : "ies"} across{" "}
                {callsignResult.windows_written.length} window
                {callsignResult.windows_written.length === 1 ? "" : "s"}
              </div>
              <p>
                {callsignResult.note} Verify on the radio itself: an incoming or monitored
                DMR ID in this batch should now resolve to its callsign and name on the
                caller-ID screen.
              </p>
            </div>
          )}

          {/* Verify / Restore — reachable from the in-memory result OR the
              newest image on disk, so a lost result (USB drop on reboot)
              doesn't strand the user. */}
          {(expectedPath || backupPath) && (
            <div className="space-y-2 rounded-md border border-slate-200 bg-slate-50 p-3 text-xs text-slate-600 dark:border-slate-700 dark:bg-slate-800/50 dark:text-slate-300">
              {usingLatest && (
                <p>
                  Using the most recent program image on disk
                  {latest?.stamp ? ` (${latest.stamp})` : ""}. If the last Program
                  looked like it failed, the radio likely still committed — it
                  reboots and drops off USB before the app gets its reply. Rescan,
                  reselect the port, then Verify.
                </p>
              )}
              {callsignResult && !expectedPath && (
                <p>
                  The call-sign database has no byte-level Verify (there's no golden
                  image to diff) — confirm it on the radio's caller-ID screen. Restore
                  writes the pre-write backup back if anything looks wrong.
                </p>
              )}
              <div className="flex flex-wrap gap-2">
                {expectedPath && (
                  <Button onClick={doVerify} disabled={!port || busy !== null}>
                    {busy === "verify" ? (
                      <Spinner className="h-3.5 w-3.5" />
                    ) : (
                      <ShieldCheck size={14} />
                    )}
                    Verify
                  </Button>
                )}
                <Button variant="ghost" onClick={doRestore} disabled={!port || busy !== null}>
                  {busy === "restore" ? (
                    <Spinner className="h-3.5 w-3.5" />
                  ) : (
                    <Undo2 size={14} />
                  )}
                  Restore backup
                </Button>
                <Button variant="ghost" onClick={refreshPorts} disabled={busy !== null}>
                  <RefreshCw size={14} />
                  Rescan ports
                </Button>
              </div>
            </div>
          )}

          {verify && (
            <div
              className={
                verify.ok
                  ? "flex items-center gap-2 rounded-md border border-emerald-200 bg-emerald-50 px-3 py-2 text-xs text-emerald-700 dark:border-emerald-900/50 dark:bg-emerald-950/40 dark:text-emerald-300"
                  : "rounded-md border border-amber-300 bg-amber-50 p-3 text-xs text-amber-800 dark:border-amber-900/50 dark:bg-amber-950/40 dark:text-amber-200"
              }
            >
              {verify.ok ? (
                <>
                  <CheckCircle2 size={14} className="shrink-0" />
                  <span>
                    Verified — every written byte matches (
                    {verify.bytes_checked.toLocaleString()} bytes checked).
                  </span>
                </>
              ) : (
                <>
                  <div className="mb-1 flex items-center gap-1.5 font-semibold">
                    <AlertTriangle size={14} /> {verify.mismatches.length} mismatch
                    {verify.mismatches.length === 1 ? "" : "es"} of{" "}
                    {verify.bytes_checked.toLocaleString()} bytes — consider Restore
                    backup.
                  </div>
                  <div className="max-h-32 overflow-auto font-mono text-[10px]">
                    {verify.mismatches.slice(0, 50).map((m, i) => (
                      <div key={i}>
                        {m.addr} expected {m.a_hex} got {m.b_hex}
                      </div>
                    ))}
                  </div>
                </>
              )}
            </div>
          )}

          {restored && (
            <div className="flex items-start gap-2 rounded-md border border-emerald-200 bg-emerald-50 px-3 py-2 text-xs text-emerald-700 dark:border-emerald-900/50 dark:bg-emerald-950/40 dark:text-emerald-300">
              <CheckCircle2 size={14} className="mt-px shrink-0" />
              <span>
                Restored {restored.windows_written.length} window
                {restored.windows_written.length === 1 ? "" : "s"} from the backup.{" "}
                {restored.note}
              </span>
            </div>
          )}
        </div>

        <div className="flex items-center justify-end gap-2 border-t border-slate-200 px-4 py-3 dark:border-slate-700">
          <Button variant="ghost" onClick={onClose}>
            Close
          </Button>
        </div>
      </div>
    </Modal>
  );
}
