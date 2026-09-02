import { useEffect, useMemo, useState, type ReactNode } from "react";
import {
  confirm as confirmDialog,
  open as openDialog,
} from "@tauri-apps/plugin-dialog";
import {
  Trash2,
  Save,
  DownloadCloud,
  UploadCloud,
  RefreshCw,
  HardDrive,
} from "lucide-react";
import clsx from "clsx";
import { toast } from "sonner";
import { api, withToast } from "../../lib/api";
import {
  mediaWriteForFormat,
  useDriverCapabilities,
} from "../../lib/radioProgramming";
import type {
  AnytoneDownloadResult,
  AnytoneImportSummary,
  PortInfo,
  RadioModel,
  RadioProfile,
  RadioSettingsRead,
  SettingField,
} from "../../lib/types";
import {
  modelBands,
  modelModes,
  modelRange,
  parseSchema,
  parseSettings,
  seedValues,
  settingsRangeErrors,
  settingsTabs,
  type SettingsValues,
} from "../../lib/profiles";
import { Button, TextInput, Select, Badge, Spinner } from "../ui";

/**
 * Pull the radio's current non-channel settings into the profile form. The
 * inverse of writing settings during programming: read the radio, decode the
 * settings, and merge them into the editor so the user can review and Save.
 * Available for the radios whose cable driver can read settings (UV-5R, TD-H3);
 * the caller passes the model-appropriate read command.
 */
function RadioSyncBar({
  profileId,
  modelLabel,
  read,
  onLoaded,
}: {
  profileId: number;
  modelLabel: string;
  read: (port: string, profileId: number) => Promise<RadioSettingsRead>;
  onLoaded: (settings: SettingsValues) => void;
}) {
  const [ports, setPorts] = useState<PortInfo[]>([]);
  const [port, setPort] = useState("");
  const [busy, setBusy] = useState(false);

  const refresh = async () => {
    try {
      const list = await api.listSerialPorts();
      setPorts(list);
      const usb = list.find((p) => p.kind === "usb");
      setPort((cur) => cur || usb?.name || list[0]?.name || "");
    } catch {
      /* surfaced on download */
    }
  };

  useEffect(() => {
    refresh();
  }, []);

  const download = async () => {
    if (!port) return;
    setBusy(true);
    const res = await withToast(read(port, profileId), {
      error: "Could not read settings from the radio",
    });
    setBusy(false);
    if (res) {
      onLoaded(res.settings);
      const { toast } = await import("sonner");
      toast.success(
        `Loaded ${res.count} setting${res.count === 1 ? "" : "s"} from the radio — review and Save to keep them.`,
      );
    }
  };

  return (
    <div className="flex flex-wrap items-end gap-2 rounded-md border border-sky-200 bg-sky-50/60 px-3 py-2.5 dark:border-sky-900/50 dark:bg-sky-950/30">
      <div className="flex-1">
        <span className="mb-1 block text-[11px] font-medium text-slate-500 dark:text-slate-400">
          Read current settings from a connected {modelLabel}
        </span>
        <div className="flex items-center gap-2">
          <Select
            className="min-w-0 flex-1"
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
          <Button variant="ghost" onClick={refresh} title="Rescan ports">
            <RefreshCw size={14} />
          </Button>
        </div>
      </div>
      <Button variant="primary" onClick={download} disabled={!port || busy}>
        {busy ? <Spinner className="h-3.5 w-3.5" /> : <DownloadCloud size={14} />}
        Download from radio
      </Button>
    </div>
  );
}

/**
 * Push this profile's saved settings to the radio. The exact inverse of
 * RadioSyncBar, and gated the same way — on the DRIVER's `write_settings`
 * capability, never on a model name.
 *
 * ⚠ It exists because the capability did not have a caller. `SettingsWriter`
 * was implemented and hardware-proven on two radios, `write_radio_settings` was
 * registered, and the only controls that reached it were the TD-H3's and the
 * AnyTone's own program dialogs — so a radio using the generic UI could read
 * its settings into this form and had no way to send them back. That is the
 * dead-write-path trap one layer up from where it was caught before: the read
 * half works, so nothing looks broken.
 *
 * It writes what is SAVED, not what is typed. The command reads the profile out
 * of the database, so an unsaved edit in this form would not go to the radio —
 * hence the Save prompt rather than a silent partial write.
 */
function WriteToRadioBar({
  profileId,
  modelLabel,
  dirty,
  neverSaved,
}: {
  profileId: number;
  modelLabel: string;
  dirty: boolean;
  /// The profile row carries no settings yet, so there is nothing to send and
  /// the command would only error. Treated like `dirty`: same button, same
  /// instruction.
  neverSaved: boolean;
}) {
  const [ports, setPorts] = useState<PortInfo[]>([]);
  const [port, setPort] = useState("");
  const [busy, setBusy] = useState(false);
  const [confirming, setConfirming] = useState(false);

  const refresh = async () => {
    try {
      const list = await api.listSerialPorts();
      setPorts(list);
      const usb = list.find((p) => p.kind === "usb");
      setPort((cur) => cur || usb?.name || list[0]?.name || "");
    } catch {
      /* surfaced on write */
    }
  };

  useEffect(() => {
    refresh();
  }, []);

  const upload = async () => {
    if (!port) return;
    setConfirming(false);
    setBusy(true);
    const res = await withToast(api.writeRadioSettings(port, profileId), {
      error: "Could not write settings to the radio",
    });
    setBusy(false);
    if (!res) return;
    const { toast } = await import("sonner");
    const n = res.fields_written;
    const applied = `Wrote ${n} setting${n === 1 ? "" : "s"} to the radio`;
    // ⚠ `note` carries the fields that were DROPPED — out of range for the
    // schema, or a select value this app cannot name — so it has to be shown
    // whether or not the write verified. Reporting "verified ✓" while silently
    // discarding the list of what never made it is worse than not reporting.
    const suffix = res.note ? ` — ${res.note}` : "";
    // Three states, not two. `verified: null` means the radio offers no
    // in-session read-back (the AnyTone reboots on commit), which is not the
    // same as a read-back that disagreed.
    if (res.verified === true) {
      toast.success(`${applied} · read back and verified ✓${suffix}`);
    } else if (res.verified === null) {
      toast.info(
        `${applied}. This radio cannot be read back in the same session, so the write is unverified${suffix}`,
      );
    } else {
      toast.warning(res.note || `${applied}, but the read-back did not confirm it.`);
    }
  };

  return (
    <div className="rounded-md border border-amber-200 bg-amber-50/60 px-3 py-2.5 dark:border-amber-900/50 dark:bg-amber-950/20">
      <div className="flex flex-wrap items-end gap-2">
        <div className="flex-1">
          <span className="mb-1 block text-[11px] font-medium text-slate-500 dark:text-slate-400">
            Write these settings to a connected {modelLabel}
          </span>
          <div className="flex items-center gap-2">
            <Select
              className="min-w-0 flex-1"
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
            <Button variant="ghost" onClick={refresh} title="Rescan ports">
              <RefreshCw size={14} />
            </Button>
          </div>
        </div>
        <Button
          variant="primary"
          onClick={() => setConfirming(true)}
          disabled={!port || busy || dirty || neverSaved}
          title={
            dirty || neverSaved
              ? "Save this profile first — the radio gets the saved values"
              : undefined
          }
        >
          {busy ? <Spinner className="h-3.5 w-3.5" /> : <UploadCloud size={14} />}
          Write to radio
        </Button>
      </div>
      {(dirty || neverSaved) && (
        <p className="mt-2 text-[11px] text-amber-700 dark:text-amber-400">
          {neverSaved
            ? "This profile has not been saved yet. The radio is written from the saved profile, so Save first."
            : "This profile has unsaved changes. The radio is written from the saved profile, so Save first."}
        </p>
      )}
      {confirming && (
        <div className="mt-2 flex flex-wrap items-center gap-2 rounded-md border border-amber-300 bg-amber-100/70 px-3 py-2 text-xs dark:border-amber-800 dark:bg-amber-900/30">
          <span className="flex-1 text-slate-700 dark:text-slate-200">
            This changes settings on the radio. Channels are left untouched, and a
            backup of the radio is written beside the app's data first.
          </span>
          <Button variant="ghost" onClick={() => setConfirming(false)}>
            Cancel
          </Button>
          <Button variant="primary" onClick={upload}>
            Write to radio
          </Button>
        </div>
      )}
    </div>
  );
}

/// The card file each media format's settings are decoded out of. A radio
/// programmed from a card gets a settings loader the moment its format has a
/// reader here — no new branch in the editor.
const CARD_SETTINGS_READERS: Record<
  string,
  (path: string) => Promise<RadioSettingsRead>
> = {
  yaesu_ft5d_sd: api.readFt5dSettingsFromBackup,
  icom_id52_icf: api.readId52SettingsFromCard,
  kenwood_thd75_sd: api.readThd75SettingsFromCard,
};

/**
 * Settings loader for a card radio. Neither the FT5D nor the ID-52 has a cable
 * settings session, so their "radio" for settings purposes is the file the
 * radio itself writes to its microSD card — the same file the codeplug export
 * patches. Nothing is written here; the values land in the form and the user
 * Saves them like any other edit.
 *
 * `format` is the model's `export_format`, which is what both the file picker
 * and the front-panel menu steps are keyed on, and `read` is the command that
 * decodes that particular file.
 */
function CardSettingsBar({
  format,
  read,
  onLoaded,
}: {
  format: string;
  read: (path: string) => Promise<RadioSettingsRead>;
  onLoaded: (settings: SettingsValues) => void;
}) {
  const [busy, setBusy] = useState(false);
  // Where the radio's own files live, if a card is mounted. Finding the card is
  // the app's job, not the operator's.
  const [card, setCard] = useState<string | undefined>(undefined);
  useEffect(() => {
    api
      .findMemoryCards(format)
      .then((cards) => setCard(cards[0]?.path))
      .catch(() => setCard(undefined));
  }, [format]);
  // Same descriptor the Program and Export dialogs use, so the file picker and
  // the front-panel menu steps are written down in exactly one place.
  const media = mediaWriteForFormat(format);

  const load = async () => {
    const picked = await openDialog({
      title: media?.pickTitle,
      multiple: false,
      directory: false,
      defaultPath: card,
      filters: media
        ? [{ name: media.filterName, extensions: media.extensions }]
        : undefined,
    });
    if (typeof picked !== "string") return;
    setBusy(true);
    const res = await withToast(read(picked), {
      error: "Could not read settings from that file",
    });
    setBusy(false);
    if (res) {
      onLoaded(res.settings as SettingsValues);
      const { toast } = await import("sonner");
      toast.success(
        `Loaded ${res.count} setting${res.count === 1 ? "" : "s"} from the card \u2014 review and Save to keep them.`,
      );
    }
  };

  return (
    <div className="flex flex-wrap items-end gap-2 rounded-md border border-sky-200 bg-sky-50/60 px-3 py-2.5 dark:border-sky-900/50 dark:bg-sky-950/30">
      <div className="flex-1">
        <span className="mb-1 block text-[11px] font-medium text-slate-500 dark:text-slate-400">
          Read current settings from the radio’s microSD card
        </span>
        <span className="block text-[11px] text-slate-400">
          {media?.before} Nothing is written here: the values land in the form
          and go back out with the codeplug when you program the radio.
        </span>
      </div>
      <Button variant="primary" onClick={load} disabled={busy}>
        {busy ? <Spinner className="h-3.5 w-3.5" /> : <HardDrive size={14} />}
        Load from microSD
      </Button>
    </div>
  );
}

/**
 * AnyTone AT-D890UV cable backup (Stage 1: read-only). The D890UV has no decoded
 * settings yet, so this panel just identifies the radio and captures a full-image
 * `.img` backup over USB — the safe foundation the decode/program path is built
 * on. Mirrors RadioSyncBar's port picker.
 */
function AnytoneBackupBar({ modelLabel }: { modelLabel: string }) {
  const [ports, setPorts] = useState<PortInfo[]>([]);
  const [port, setPort] = useState("");
  const [busy, setBusy] = useState<"idle" | "identify" | "download" | "import">(
    "idle",
  );
  const [status, setStatus] = useState<string | null>(null);
  const [probe, setProbe] = useState<AnytoneDownloadResult | null>(null);
  // Library import of the last download (channels / zones→lists / TGs).
  const [importSummary, setImportSummary] =
    useState<AnytoneImportSummary | null>(null);

  const refresh = async () => {
    try {
      const list = await api.listSerialPorts();
      setPorts(list);
      // Keep the current port only if it still exists; otherwise fall back to a
      // USB port (the radio may have re-enumerated under a new name).
      setPort((cur) => {
        if (cur && list.some((p) => p.name === cur)) return cur;
        const usb = list.find((p) => p.kind === "usb");
        return usb?.name || list[0]?.name || "";
      });
    } catch {
      /* surfaced on action */
    }
  };

  useEffect(() => {
    refresh();
  }, []);

  const cleanIdent = (s: string) => s.replace(/\.+/g, "").trim();

  const identify = async () => {
    if (!port) return;
    setBusy("identify");
    // No custom error message — surface the real backend error.
    const res = await withToast(api.identifyRadio("anytone_atd890uv", port));
    setBusy("idle");
    if (!res) {
      await refresh(); // a vanished/renamed port self-corrects on the next try
      return;
    }
    setStatus(`Identified: ${cleanIdent(res.ident_ascii ?? res.ident_hex)}`);
    const { toast } = await import("sonner");
    toast.success(`Radio identified as ${cleanIdent(res.ident_ascii ?? res.ident_hex)}.`);
  };

  const download = async () => {
    if (!port) return;
    setBusy("download");
    // No custom error message — surface the real backend error.
    const res = await withToast(api.downloadAnytoneImage(port));
    setBusy("idle");
    if (!res) {
      await refresh();
      return;
    }
    setProbe(res);
    setImportSummary(null); // a fresh download hasn't been imported yet
    const responded = res.regions.filter((r) => r.read > 0).length;
    const { toast } = await import("sonner");
    if (responded > 0) {
      const decoded =
        res.channels.length > 0
          ? ` · decoded ${res.channels.length} channels`
          : "";
      setStatus(
        `Captured ${res.image_bytes} bytes from ${responded}/${res.regions.length} regions${decoded} → ${res.backup_path ?? "(not saved)"}`,
      );
      toast.success(
        res.channels.length > 0
          ? `Decoded ${res.channels.length} channels from ${responded} regions.`
          : `${responded} of ${res.regions.length} probe regions responded.`,
      );
    } else {
      setStatus("No probe region responded — the D890UV map differs from the D868/D878 hypothesis.");
      toast.warning("No region responded; see the probe report below.");
    }
  };

  // Import the decoded download into the library: channels → channel library,
  // zones → channel lists, DMR contacts → talkgroups. Pure DB work (no radio
  // session); everything dedupes so re-importing is safe.
  const importToLibrary = async () => {
    if (!probe || probe.channels.length === 0) return;
    setBusy("import");
    const res = await withToast(
      api.importAnytoneDownload(probe.channels, probe.zones, probe.contacts),
    );
    setBusy("idle");
    if (!res) return;
    setImportSummary(res);
    const skipped =
      res.channels_skipped + res.lists_skipped + res.talkgroups_skipped;
    const { toast } = await import("sonner");
    toast.success(
      `Imported ${res.channels_added} channels, ${res.lists_added} lists, ` +
        `${res.talkgroups_added} talkgroups` +
        (skipped > 0 ? ` (${skipped} already in the library)` : "") +
        ".",
    );
  };

  return (
    <div className="space-y-2 rounded-md border border-sky-200 bg-sky-50/60 px-3 py-2.5 dark:border-sky-900/50 dark:bg-sky-950/30">
      <span className="block text-[11px] font-medium text-slate-500 dark:text-slate-400">
        Connect a {modelLabel} over USB to identify it and capture a full backup.
        Reading is non-destructive. A gated write test (round-trip no-op) is
        available below.
      </span>
      <div className="flex flex-wrap items-end gap-2">
        <div className="flex flex-1 items-center gap-2">
          <Select
            className="min-w-0 flex-1"
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
          <Button variant="ghost" onClick={refresh} title="Rescan ports">
            <RefreshCw size={14} />
          </Button>
        </div>
        <Button
          variant="ghost"
          onClick={identify}
          disabled={!port || busy !== "idle"}
        >
          {busy === "identify" ? <Spinner className="h-3.5 w-3.5" /> : null}
          Identify
        </Button>
        <Button
          variant="primary"
          onClick={download}
          disabled={!port || busy !== "idle"}
        >
          {busy === "download" ? (
            <Spinner className="h-3.5 w-3.5" />
          ) : (
            <DownloadCloud size={14} />
          )}
          Back up radio
        </Button>
      </div>
      {status && (
        <p className="break-all text-[11px] text-slate-500 dark:text-slate-400">
          {status}
        </p>
      )}
      {probe && (
        <div className="space-y-1 rounded border border-slate-200 bg-white/70 p-2 font-mono text-[10px] dark:border-slate-700 dark:bg-slate-900/40">
          {probe.regions.map((r) => (
            <div key={r.name} className="break-all">
              <span
                className={
                  r.read > 0
                    ? "text-emerald-600 dark:text-emerald-400"
                    : "text-red-500"
                }
              >
                {r.read > 0 ? "✓" : "✗"} {r.name} @ {r.addr}
              </span>{" "}
              <span className="text-slate-500 dark:text-slate-400">
                {r.read}/{r.requested}B
                {r.read > 0 && !r.checksum_ok ? " · checksum?" : ""}
                {r.checksum_formula ? ` · cksum: ${r.checksum_formula}` : ""}
                {r.error ? ` · ${r.error}` : ""}
              </span>
              {r.preview && (
                <div className="text-slate-400 dark:text-slate-500">
                  {r.preview}
                </div>
              )}
            </div>
          ))}
        </div>
      )}
      {probe && probe.channels.length > 0 && (
        <div className="space-y-2">
          <div className="max-h-72 overflow-auto rounded border border-slate-200 bg-white/70 dark:border-slate-700 dark:bg-slate-900/40">
            <table className="w-full text-[10px]">
              <thead className="sticky top-0 bg-slate-100 text-left text-slate-500 dark:bg-slate-800 dark:text-slate-400">
                <tr>
                  <th className="px-1.5 py-1">#</th>
                  <th className="px-1.5 py-1">Name</th>
                  <th className="px-1.5 py-1 text-right">RX (MHz)</th>
                  <th className="px-1.5 py-1">Shift</th>
                  <th className="px-1.5 py-1">Mode</th>
                  <th className="px-1.5 py-1">Power</th>
                  <th className="px-1.5 py-1">BW</th>
                  <th className="px-1.5 py-1">Tone / CC·TS·TG</th>
                </tr>
              </thead>
              <tbody className="font-mono">
                {probe.channels.map((c) => (
                  <tr
                    key={c.index}
                    className="border-t border-slate-100 dark:border-slate-800"
                  >
                    <td className="px-1.5 py-0.5 text-slate-400">{c.index}</td>
                    <td className="px-1.5 py-0.5 font-sans">{c.name}</td>
                    <td className="px-1.5 py-0.5 text-right">
                      {c.rx_mhz.toFixed(4)}
                    </td>
                    <td className="px-1.5 py-0.5">{c.shift || "—"}</td>
                    <td className="px-1.5 py-0.5">{c.mode}</td>
                    <td className="px-1.5 py-0.5">{c.power}</td>
                    <td className="px-1.5 py-0.5">{c.bandwidth}</td>
                    <td className="px-1.5 py-0.5">
                      {c.color_code != null
                        ? `CC${c.color_code} · TS${c.time_slot} · TG ${
                            c.contact_name ?? `#${c.contact_index}`
                          }`
                        : c.tone}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <Button
              variant="primary"
              onClick={importToLibrary}
              disabled={busy !== "idle"}
            >
              {busy === "import" ? (
                <Spinner className="h-3.5 w-3.5" />
              ) : (
                <DownloadCloud size={14} />
              )}
              Import to library
            </Button>
            <span className="text-[11px] text-slate-500 dark:text-slate-400">
              {importSummary
                ? `Added ${importSummary.channels_added} channels, ` +
                  `${importSummary.lists_added} lists, ` +
                  `${importSummary.talkgroups_added} talkgroups · skipped ` +
                  `${
                    importSummary.channels_skipped +
                    importSummary.lists_skipped +
                    importSummary.talkgroups_skipped
                  } already present`
                : `Adds the ${probe.channels.length} channels to the channel library, ` +
                  `zones as channel lists, and DMR talkgroups. Safe to repeat — ` +
                  `existing entries are skipped.`}
            </span>
          </div>
          {probe.zones.map((z) => (
            <div
              key={z.index}
              className="rounded border border-slate-200 bg-white/70 p-2 text-[11px] dark:border-slate-700 dark:bg-slate-900/40"
            >
              <span className="font-medium text-slate-600 dark:text-slate-300">
                Zone {z.index}
                {z.name ? ` · ${z.name}` : ""}
              </span>{" "}
              <span className="text-slate-500 dark:text-slate-400">
                ({z.channels.length} channels): {z.channels.join(", ")}
              </span>
            </div>
          ))}
        </div>
      )}


    </div>
  );
}

type Tab = "capabilities" | "settings";

function SpecRow({ label, value }: { label: string; value: ReactNode }) {
  return (
    <div className="flex items-center justify-between gap-4 border-b border-slate-100 py-1.5 text-xs last:border-b-0 dark:border-slate-700/60">
      <span className="text-slate-500 dark:text-slate-400">{label}</span>
      <span className="text-right font-medium text-slate-800 dark:text-slate-100">
        {value}
      </span>
    </div>
  );
}

function Capabilities({ model }: { model: RadioModel }) {
  const yn = (b: boolean) => (b ? "Yes" : "No");
  return (
    <div className="grid grid-cols-1 gap-x-8 gap-y-0 md:grid-cols-2">
      <div>
        <SpecRow label="Manufacturer" value={model.manufacturer} />
        <SpecRow label="Model" value={model.model} />
        <SpecRow
          label="Bands"
          value={
            <span className="flex flex-wrap justify-end gap-1">
              {modelBands(model).map((b) => (
                <Badge key={b}>{b}</Badge>
              ))}
            </span>
          }
        />
        <SpecRow label="Modes" value={modelModes(model).join(", ")} />
        <SpecRow label="APRS" value={yn(model.aprs_capable)} />
        <SpecRow label="Transmit range" value={modelRange(model, "tx") ?? "—"} />
        {modelRange(model, "rx") && (
          <SpecRow label="Receive range" value={modelRange(model, "rx")} />
        )}
      </div>
      <div>
        <SpecRow label="Memory channels" value={model.memory_channels ?? "—"} />
        <SpecRow
          label="Zones"
          value={
            model.zones_supported
              ? `Up to ${model.max_zones ?? "?"}${
                  model.channels_per_zone
                    ? ` · ${model.channels_per_zone}/zone`
                    : ""
                }`
              : "Not supported"
          }
        />
        <SpecRow
          label="Scan lists"
          value={
            model.scan_lists_supported
              ? `Up to ${model.max_scan_lists ?? "?"}`
              : "Not supported"
          }
        />
        <SpecRow label="Banks" value={yn(model.banks_supported)} />
        <SpecRow label="Max name length" value={model.max_name_length ?? "—"} />
        <SpecRow label="Export format" value={model.export_format ?? "—"} />
        <SpecRow label="Connection" value={model.connection_type ?? "—"} />
      </div>
    </div>
  );
}

/**
 * The two-column field grid. Takes a flat list, so it draws either a whole
 * schema — headings included — or the fields of one sub-tab, which have had
 * their heading lifted into the tab button.
 */
function SettingsGrid({
  fields,
  values,
  errors,
  onChange,
}: {
  fields: SettingField[];
  values: SettingsValues;
  errors: Record<string, string>;
  onChange: (key: string, v: string | number | boolean) => void;
}) {
  return (
    <div className="grid grid-cols-1 items-end gap-4 sm:grid-cols-2">
      {fields.map((f) =>
        f.type === "section" ? (
          <h4
            key={f.key}
            className="mt-2 border-b border-slate-200 pb-1 text-xs font-semibold uppercase tracking-wide text-slate-500 first:mt-0 sm:col-span-2 dark:border-slate-700 dark:text-slate-400"
          >
            {f.label}
          </h4>
        ) : (
          <SettingsField
            key={f.key}
            field={f}
            value={values[f.key] ?? ""}
            error={errors[f.key]}
            onChange={(v) => onChange(f.key, v)}
          />
        ),
      )}
    </div>
  );
}

function SettingsField({
  field,
  value,
  error,
  onChange,
}: {
  field: SettingField;
  value: string | number | boolean;
  error?: string;
  onChange: (v: string | number | boolean) => void;
}) {
  if (field.type === "boolean") {
    return (
      <label className="flex items-center gap-2 text-xs text-slate-700 dark:text-slate-200">
        <input
          type="checkbox"
          checked={Boolean(value)}
          onChange={(e) => onChange(e.target.checked)}
        />
        {field.label}
      </label>
    );
  }

  const control =
    field.type === "select" ? (
      <Select
        value={String(value)}
        onChange={(e) => onChange(e.target.value)}
      >
        {/* A value the option list does not contain would otherwise display as
            the first option, which is a lie in both directions: an unset field
            would read as a real setting, and a value the radio holds but this
            app cannot name would read as a different one. Both happen — a card
            radio's profile starts blank on purpose, and an unrecognised stored
            value decodes to its raw number. */}
        {!(field.options ?? []).includes(String(value)) && (
          <option value={String(value)}>
            {String(value) === "" ? "— not set —" : `${value} (unrecognised)`}
          </option>
        )}
        {(field.options ?? []).map((o) => (
          <option key={o} value={o}>
            {o}
          </option>
        ))}
      </Select>
    ) : field.type === "integer" ? (
      <TextInput
        type="number"
        min={field.min}
        max={field.max}
        aria-invalid={error ? true : undefined}
        className={error ? "border-rose-500 dark:border-rose-500" : undefined}
        value={value === "" ? "" : String(value)}
        onChange={(e) => {
          const n = e.target.value === "" ? "" : Number(e.target.value);
          onChange(n === "" || Number.isNaN(n) ? "" : n);
        }}
      />
    ) : (
      <TextInput
        maxLength={field.max_length}
        value={String(value ?? "")}
        onChange={(e) => onChange(e.target.value)}
      />
    );

  return (
    <label className="flex flex-col gap-1">
      <span className="text-[11px] font-medium text-slate-500 dark:text-slate-400">
        {field.label}
        {field.type === "integer" && field.min != null && field.max != null && (
          <span className="ml-1 text-slate-400">
            ({field.min}–{field.max})
          </span>
        )}
      </span>
      {control}
      {/* The range is already in the label; this says what to do about it, and
          is what stops the value being saved at all (#87). */}
      {error && (
        <span className="text-[11px] text-rose-600 dark:text-rose-400">{error}</span>
      )}
    </label>
  );
}

export function ProfileEditor({
  profile,
  model,
  onSaved,
  onDeleted,
}: {
  profile: RadioProfile;
  model: RadioModel | undefined;
  onSaved: (updated: RadioProfile) => void;
  onDeleted: (id: number) => void;
}) {
  const [tab, setTab] = useState<Tab>("settings");
  const [saving, setSaving] = useState(false);

  const fields = useMemo(() => parseSchema(model), [model]);
  // Non-null only for the radios whose settings are split across sub-tabs —
  // see `settingsTabs`. Null keeps the original single scroll.
  const subTabs = useMemo(() => settingsTabs(fields), [fields]);
  const [subTab, setSubTab] = useState<string | null>(null);
  const openSubTab =
    subTabs?.find((t) => t.key === subTab) ?? subTabs?.[0] ?? null;
  // Programmed from its own memory card, which means its settings are patched
  // into a file that already holds the operator's — see the seeding note below.
  //
  // ⚠ Keyed on the MEDIA-WRITE map, not on CARD_SETTINGS_READERS. Whether a
  // settings *reader* happens to be wired yet says nothing about whether this
  // radio's file already holds the operator's settings — and the state where
  // they diverge (a card radio with a schema but no decoder yet) is the normal
  // intermediate state of every new-radio branch. Keying on the reader made
  // that state seed ~300 schema defaults into a file the radio itself wrote.
  // (#90)
  const isCardRadio = mediaWriteForFormat(model?.export_format ?? null) !== null;
  // Separately: whether "Download from radio" can be offered at all.
  const cardReader = model?.export_format
    ? CARD_SETTINGS_READERS[model.export_format]
    : undefined;
  // What this model's driver can actually do — gates the read-from-radio bar.
  const caps = useDriverCapabilities(model?.driver_key ?? null);

  // Form state, re-seeded whenever a different profile is selected.
  const [name, setName] = useState(profile.display_name);
  const [notes, setNotes] = useState(profile.notes ?? "");
  const [values, setValues] = useState<SettingsValues>({});
  // What the form started from — the profile as stored, plus anything later
  // read off the radio or its card. A value that is out of range in HERE came
  // from a radio rather than from the operator, and blocking the save over it
  // would strand a profile they never typed into (see below).
  const [baseline, setBaseline] = useState<SettingsValues>({});
  // What the DATABASE holds, which is a different question from `baseline`.
  // `baseline` means "not typed by the operator" and deliberately absorbs a
  // read from the radio, so it cannot answer "is this profile saved?" — and
  // `write_radio_settings` sends the SAVED row, not the form.
  const [saved, setSaved] = useState<SettingsValues>({});
  const [lastId, setLastId] = useState<number | null>(null);
  if (profile.id !== lastId) {
    setName(profile.display_name);
    setNotes(profile.notes ?? "");
    // A card radio's settings are patched into the operator's own file, so this
    // form must not invent values for fields they have never set — see
    // `seedValues`. Read them off the card first, or leave them alone.
    const seeded = seedValues(
      fields,
      parseSettings(profile.non_channel_settings),
      !isCardRadio,
    );
    setValues(seeded);
    setBaseline(seeded);
    setSaved(seeded);
    setLastId(profile.id);
    setTab("settings");
    setSubTab(null);
  }

  const setValue = (key: string, v: string | number | boolean) =>
    setValues((s) => ({ ...s, [key]: v }));

  // `write_radio_settings` sends the profile as STORED, so anything the form
  // holds that the database does not must block the write.
  //
  // ⚠ This compares against `saved`, NOT `baseline`. `baseline` absorbs a read
  // from the radio on purpose, so using it here made the button live again the
  // instant "Download from radio" finished — and that write would have sent the
  // PREVIOUSLY SAVED values straight back over the ones just read, while the
  // form went on displaying the radio's. The one path that most needs the guard
  // was the one path it did not cover.
  const dirty = useMemo(
    () =>
      name !== profile.display_name ||
      notes !== (profile.notes ?? "") ||
      fields.some((f) => values[f.key] !== saved[f.key]),
    [name, notes, values, saved, fields, profile],
  );

  // Values that came off the radio (or its card) are the new starting point,
  // not an edit — a radio is allowed to hold a value this app's schema does not
  // describe, and it must stay saveable.
  const loadFromRadio = (loaded: SettingsValues) => {
    setValues((v) => ({ ...v, ...loaded }));
    setBaseline((b) => ({ ...b, ...loaded }));
  };

  // Values the schema says the radio cannot take. Kept live rather than
  // computed on save so the message appears as the operator types, and so the
  // sub-tab holding an offending field can be marked (#87).
  const rangeErrors = useMemo(() => {
    const out: Record<string, string> = {};
    for (const [key, message] of Object.entries(
      settingsRangeErrors(fields, values),
    )) {
      // A value that was already in the profile is one the radio gave us, and
      // it is not going to be programmed — say that, rather than issuing an
      // instruction about a field the operator never touched.
      out[key] =
        values[key] === baseline[key]
          ? `${message} This one is not written to the radio.`
          : message;
    }
    return out;
  }, [fields, values, baseline]);

  const save = async () => {
    if (!name.trim()) return;
    // Saving an out-of-range value is how it reached the radio: the encoder
    // casts it down to a byte, so 300 in a 0–24 field was programmed as 44
    // (#87). Only a value the operator TYPED blocks the save, and it can be
    // pointed at — including on a sub-tab that is not on screen. One that was
    // already in the profile is marked but saveable: it came off a radio, and
    // the write paths drop it with a note rather than programming it.
    const firstBad = fields.find(
      (f) => rangeErrors[f.key] && values[f.key] !== baseline[f.key],
    );
    if (firstBad) {
      const tab = subTabs?.find((t) => t.fields.some((f) => f.key === firstBad.key));
      if (tab) setSubTab(tab.key);
      toast.error(`${firstBad.label}: ${rangeErrors[firstBad.key]}`);
      return;
    }
    setSaving(true);
    const updated = await withToast(
      api.updateRadioProfile(profile.id, {
        display_name: name.trim(),
        radio_model_id: profile.radio_model_id,
        non_channel_settings: JSON.stringify(values),
        notes: notes.trim() || null,
      }),
      { success: "Profile saved" },
    );
    setSaving(false);
    if (updated) {
      setSaved(values);
      onSaved(updated);
    }
  };

  const remove = async () => {
    const ok = await confirmDialog(
      `Delete profile “${profile.display_name}”? This cannot be undone.`,
      { title: "Delete profile", kind: "warning" },
    );
    if (!ok) return;
    const res = await withToast(api.deleteRadioProfile(profile.id), {
      success: "Profile deleted",
    });
    if (res !== undefined) onDeleted(profile.id);
  };

  return (
    <div className="flex h-full flex-col">
      {/* Header */}
      <div className="flex items-start justify-between gap-4 border-b border-slate-200 px-5 py-3 dark:border-slate-700">
        <div className="min-w-0 flex-1">
          <TextInput
            className="w-full max-w-md text-sm font-semibold"
            value={name}
            onChange={(e) => setName(e.target.value)}
          />
          <div className="mt-1.5 flex items-center gap-2 text-xs text-slate-500 dark:text-slate-400">
            <Badge className="bg-sky-100 text-sky-700 dark:bg-sky-950 dark:text-sky-300">
              {model?.display_name ?? "Unknown model"}
            </Badge>
            {model && <span>{modelModes(model).join(" · ")}</span>}
          </div>
        </div>
        <div className="flex items-center gap-2">
          <Button variant="danger" onClick={remove}>
            <Trash2 size={14} /> Delete
          </Button>
          <Button variant="primary" onClick={save} disabled={saving || !name.trim()}>
            <Save size={14} /> Save
          </Button>
        </div>
      </div>

      {/* Tabs */}
      <div className="flex gap-1 border-b border-slate-200 px-4 pt-2 dark:border-slate-700">
        {(["settings", "capabilities"] as Tab[]).map((t) => (
          <button
            key={t}
            onClick={() => setTab(t)}
            className={clsx(
              "rounded-t-md px-3 py-1.5 text-xs font-medium capitalize transition-colors",
              tab === t
                ? "border-b-2 border-sky-600 text-sky-700 dark:text-sky-300"
                : "text-slate-500 hover:text-slate-700 dark:text-slate-400 dark:hover:text-slate-200",
            )}
          >
            {t}
          </button>
        ))}
      </div>

      {/* Body */}
      <div className="flex-1 overflow-auto p-5">
        {!model ? (
          <p className="text-xs text-red-500">
            The radio model for this profile could not be found.
          </p>
        ) : tab === "capabilities" ? (
          <Capabilities model={model} />
        ) : (
          <div className="max-w-2xl space-y-5">
            {/* Read-from-radio is offered when the DRIVER says it can read
                settings (3.7), not when the model name is on a list — a new
                radio picks this up the moment it implements SettingsReader. */}
            {caps?.read_settings && fields.length > 0 && (
              <RadioSyncBar
                profileId={profile.id}
                modelLabel={model.display_name}
                read={api.readRadioSettings}
                onLoaded={loadFromRadio}
              />
            )}
            {/* The inverse, gated on the driver's write capability for the same
                reason. Card radios never get it: their settings are patched
                into the file the export writes, not sent over a cable.
                ⚠ And only for radios on the GENERIC programming UI. A radio
                with a bespoke dialog already offers this write there, in a
                place that can speak to what makes it unusual — the AnyTone's
                settings commit reboots the radio and re-enumerates USB, so it
                reports `verified: null` plus an `expected_path` to diff in a
                fresh session, and a generic bar would show neither. */}
            {caps?.write_settings &&
              fields.length > 0 &&
              (model.programming_ui ?? "generic") === "generic" && (
                <WriteToRadioBar
                  profileId={profile.id}
                  modelLabel={model.display_name}
                  dirty={dirty}
                  neverSaved={!profile.non_channel_settings}
                />
              )}
            {/* A card radio's settings come off its microSD rather than a
                cable, so it gets a file picker where the others get a port
                picker. Keyed on the export format — the same key that names the
                file and the menu steps — with the decode command beside it,
                since each radio's card file is its own format. */}
            {cardReader && model.export_format && fields.length > 0 && (
              <CardSettingsBar
                format={model.export_format}
                read={cardReader}
                onLoaded={loadFromRadio}
              />
            )}
            {/* Stays keyed to the AnyTone on purpose: this bar drives
                `download_anytone_image`, a region probe feeding the library
                importer, which is deliberately NOT part of the generic
                download_image capability (see the note in registry.rs). */}
            {model.driver_key === "anytone_atd890uv" && (
              <AnytoneBackupBar modelLabel={model.display_name} />
            )}
            {fields.length === 0 ? (
              <p className="text-xs text-slate-400">
                This model has no configurable non-channel settings.
              </p>
            ) : openSubTab ? (
              /* Split across sub-tabs. Every field's value stays in `values`
                 whichever tab is open, so this changes what is drawn and
                 nothing about what is saved. */
              <div className="space-y-3">
                <div className="flex flex-wrap gap-1">
                  {subTabs!.map((t) => (
                    <button
                      key={t.key}
                      onClick={() => setSubTab(t.key)}
                      className={clsx(
                        "rounded-md px-2.5 py-1 text-[11px] font-medium transition-colors",
                        t.key === openSubTab.key
                          ? "bg-sky-600 text-white"
                          : "bg-slate-100 text-slate-600 hover:bg-slate-200 dark:bg-slate-800 dark:text-slate-300 dark:hover:bg-slate-700",
                      )}
                    >
                      {t.label}
                      {t.fields.some((f) => rangeErrors[f.key]) && (
                        <span
                          className="ml-1 text-rose-500 dark:text-rose-400"
                          title="A setting on this tab is outside the range the radio accepts"
                        >
                          ●
                        </span>
                      )}
                    </button>
                  ))}
                </div>
                <SettingsGrid
                  fields={openSubTab.fields}
                  values={values}
                  errors={rangeErrors}
                  onChange={setValue}
                />
              </div>
            ) : (
              <SettingsGrid
                fields={fields}
                values={values}
                errors={rangeErrors}
                onChange={setValue}
              />
            )}

            <div className="space-y-1.5 pt-2">
              <h3 className="text-[11px] font-semibold uppercase tracking-wide text-slate-400">
                Notes
              </h3>
              <textarea
                className="w-full rounded-md border border-slate-300 bg-white px-2.5 py-1.5 text-xs text-slate-800 focus:border-sky-500 focus:outline-none focus:ring-1 focus:ring-sky-500 dark:border-slate-600 dark:bg-slate-900 dark:text-slate-100"
                rows={3}
                placeholder="Notes about this profile…"
                value={notes}
                onChange={(e) => setNotes(e.target.value)}
              />
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
