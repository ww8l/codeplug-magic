import { useEffect, useMemo, useState, type ReactNode } from "react";
import { confirm as confirmDialog } from "@tauri-apps/plugin-dialog";
import { Trash2, Save, DownloadCloud, RefreshCw } from "lucide-react";
import clsx from "clsx";
import { api, withToast } from "../../lib/api";
import type {
  AnytoneChannelEdit,
  AnytoneDecodedChannel,
  AnytoneDownloadResult,
  AnytoneImportSummary,
  AnytoneRoundtripResult,
  AnytoneSubTone,
  AnytoneWriteChannelsResult,
  AnytoneWriteFieldResult,
  AnytoneSlotReadResult,
  AnytoneDumpResult,
  AnytoneDumpDiff,
  PortInfo,
  RadioModel,
  RadioProfile,
  RadioSettingsRead,
  SettingField,
} from "../../lib/types";
import {
  modelBands,
  modelModes,
  parseSchema,
  parseSettings,
  seedValues,
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

// ---- Stage 3 encoder write panel: form model + decoded-channel mapping ----
// The form mirrors `AnytoneChannelEdit` with string-typed inputs. Label arrays are
// index-aligned with the mode-byte bit values, so `indexOf` on a decoded label is
// the encode value.
const ANYTONE_POWER = ["Low", "Medium", "High", "Turbo"];
const ANYTONE_MODES = ["FM", "DMR", "FM+DMR RX", "DMR+FM RX"];

type ToneKind = "none" | "ctcss" | "dcs";
interface EncForm {
  name: string;
  rx: string;
  tx: string;
  power: string; // "0".."3"
  wide: boolean;
  mode: string; // "0".."3"
  toneTxKind: ToneKind;
  toneTxVal: string;
  toneRxKind: ToneKind;
  toneRxVal: string;
  colorCode: string;
  timeSlot: string;
  contactIndex: string;
}

/** One side of a decoded tone label ("CTCSS 123.0" / "DCS 411") → form fields.
 * DCS labels carry the printed octal code — keep the digits verbatim. */
function parseToneSide(s: string): { kind: ToneKind; value: string } {
  const c = s.match(/CTCSS ([\d.]+)/);
  if (c) return { kind: "ctcss", value: c[1] };
  const d = s.match(/DCS ([0-7]+)/);
  if (d) return { kind: "dcs", value: d[1] };
  return { kind: "none", value: "" };
}

/**
 * Prefill the edit form from a decoded channel — the inverse of the decoder's
 * display labels. TX frequency is reconstructed from RX + the signed shift string;
 * the collapsed tone label means TX and RX carry the same tone.
 */
function formFromChannel(c: AnytoneDecodedChannel): EncForm {
  const shift = parseFloat(c.shift || "0") || 0;
  let tx = { kind: "none" as ToneKind, value: "" };
  let rx = tx;
  if (c.tone && c.tone !== "—") {
    if (/TX |RX /.test(c.tone)) {
      const txm = c.tone.match(/TX ([^/]+)/);
      const rxm = c.tone.match(/RX (.+)$/);
      if (txm) tx = parseToneSide(txm[1]);
      if (rxm) rx = parseToneSide(rxm[1]);
    } else {
      tx = rx = parseToneSide(c.tone);
    }
  }
  return {
    name: c.name,
    rx: c.rx_mhz.toFixed(5),
    tx: (c.rx_mhz + shift).toFixed(5),
    power: String(Math.max(0, ANYTONE_POWER.indexOf(c.power))),
    wide: c.bandwidth === "25 kHz",
    mode: String(Math.max(0, ANYTONE_MODES.indexOf(c.mode))),
    toneTxKind: tx.kind,
    toneTxVal: tx.value,
    toneRxKind: rx.kind,
    toneRxVal: rx.value,
    colorCode: String(c.color_code ?? 1),
    timeSlot: String(c.time_slot ?? 1),
    contactIndex: String(c.contact_index ?? 0),
  };
}

/** Form → backend edit, or a human-readable validation error. */
function editFromForm(f: EncForm): AnytoneChannelEdit | string {
  const rx = Number(f.rx);
  const tx = Number(f.tx);
  if (!Number.isFinite(rx) || rx <= 0) return "RX frequency is not a valid MHz value";
  if (!Number.isFinite(tx) || tx <= 0) return "TX frequency is not a valid MHz value";
  const subTone = (kind: ToneKind, val: string): AnytoneSubTone | null => {
    if (kind === "none") return null;
    // DCS values are the printed octal code ("411"); the radio wants the raw
    // binary u16 (411₈ = 265).
    return kind === "ctcss"
      ? { kind: "ctcss", value: Number(val) }
      : { kind: "dcs", value: parseInt(val, 8) };
  };
  const mode = Number(f.mode);
  if (mode === 0) {
    for (const [kind, val, side] of [
      [f.toneTxKind, f.toneTxVal, "TX"],
      [f.toneRxKind, f.toneRxVal, "RX"],
    ] as const) {
      if (kind === "ctcss" && (!Number.isFinite(Number(val)) || val === ""))
        return `${side} tone value is not a number`;
      if (kind === "dcs" && !/^[0-7]{1,3}$/.test(val))
        return `${side} DCS code must be 1-3 octal digits (0-7)`;
    }
  }
  return {
    name: f.name,
    rx_mhz: rx,
    tx_mhz: tx,
    power: Number(f.power),
    wide: f.wide,
    mode,
    tone: {
      tx: subTone(f.toneTxKind, f.toneTxVal),
      rx: subTone(f.toneRxKind, f.toneRxVal),
    },
    color_code: Number(f.colorCode) || 0,
    time_slot: Number(f.timeSlot) || 1,
    contact_index: Number(f.contactIndex) || 0,
  };
}

/** One-line human summary of what the form will write, for the verify compare. */
function formSummary(f: EncForm): string {
  const mode = Number(f.mode);
  const base = `${f.name} · RX ${f.rx} · TX ${f.tx} · ${ANYTONE_POWER[Number(f.power)]} · ${
    f.wide ? "25 kHz" : "12.5 kHz"
  } · ${ANYTONE_MODES[mode]}`;
  if (mode !== 0)
    return `${base} · CC${f.colorCode} · TS${f.timeSlot} · TG #${f.contactIndex}`;
  const side = (k: ToneKind, v: string) =>
    k === "none" ? "—" : k === "ctcss" ? `CTCSS ${v}` : `DCS ${v}`;
  return `${base} · TX ${side(f.toneTxKind, f.toneTxVal)} / RX ${side(f.toneRxKind, f.toneRxVal)}`;
}

/** Matching one-line summary of a decoded channel read back from the radio. */
function channelSummary(c: AnytoneDecodedChannel): string {
  const tail =
    c.color_code != null
      ? `CC${c.color_code} · TS${c.time_slot} · TG #${c.contact_index}`
      : c.tone;
  return `${c.name} · RX ${c.rx_mhz.toFixed(5)} · shift ${c.shift || "—"} · ${c.power} · ${c.bandwidth} · ${c.mode} · ${tail}`;
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
  const [busy, setBusy] = useState<
    | "idle"
    | "identify"
    | "download"
    | "import"
    | "roundtrip"
    | "writefield"
    | "verifyslot"
    | "restoreslot"
    | "dump"
    | "diff"
    | "encload"
    | "encwrite"
    | "encverify"
  >("idle");
  const [status, setStatus] = useState<string | null>(null);
  const [probe, setProbe] = useState<AnytoneDownloadResult | null>(null);
  // Library import of the last download (channels / zones→lists / TGs).
  const [importSummary, setImportSummary] =
    useState<AnytoneImportSummary | null>(null);
  // Stage 3 write validation — gated behind an explicit ack. The round-trip and
  // the persistence flow share the slot input and the acknowledgement checkbox.
  const [rtSlot, setRtSlot] = useState("120");
  const [rtAck, setRtAck] = useState(false);
  const [roundtrip, setRoundtrip] = useState<AnytoneRoundtripResult | null>(
    null,
  );
  // Persistence test: the write result (holds the backup path for restore) and
  // the most recent fresh read used to verify whether the change stuck.
  const [writeField, setWriteField] = useState<AnytoneWriteFieldResult | null>(
    null,
  );
  const [verifyRead, setVerifyRead] = useState<AnytoneSlotReadResult | null>(
    null,
  );
  // Integrity investigation: the two most recent deterministic dumps + the diff.
  const [dumps, setDumps] = useState<AnytoneDumpResult[]>([]);
  const [dumpDiff, setDumpDiff] = useState<AnytoneDumpDiff[] | null>(null);
  // Stage 3 encoder write: its own slot/ack (independent of the TS-flip test), the
  // loaded slot read (prefills + gates the form), the edit form, the write result,
  // and the fresh-session verify read.
  const [encSlot, setEncSlot] = useState("120");
  const [encAck, setEncAck] = useState(false);
  const [encRead, setEncRead] = useState<AnytoneSlotReadResult | null>(null);
  const [encForm, setEncForm] = useState<EncForm | null>(null);
  const [encWriteRes, setEncWriteRes] =
    useState<AnytoneWriteChannelsResult | null>(null);
  const [encVerify, setEncVerify] = useState<AnytoneSlotReadResult | null>(null);

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
    const res = await withToast(api.identifyAnytone(port));
    setBusy("idle");
    if (!res) {
      await refresh(); // a vanished/renamed port self-corrects on the next try
      return;
    }
    setStatus(`Identified: ${cleanIdent(res.ident_ascii)}`);
    const { toast } = await import("sonner");
    toast.success(`Radio identified as ${cleanIdent(res.ident_ascii)}.`);
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

  const runRoundtrip = async () => {
    const slot = Number(rtSlot);
    if (!port || !Number.isInteger(slot) || slot < 1) return;
    setBusy("roundtrip");
    setRoundtrip(null);
    // No custom error message — surface the real backend error.
    const res = await withToast(api.roundtripWriteAnytone(port, slot));
    setBusy("idle");
    if (!res) {
      await refresh();
      return;
    }
    setRoundtrip(res);
    const { toast } = await import("sonner");
    if (res.matched) {
      toast.success(
        `Round-trip OK: slot ${res.slot} read back byte-for-byte identical.`,
      );
    } else {
      toast.error(
        `Round-trip MISMATCH on slot ${res.slot} — restore from ${res.backup_path}.`,
      );
    }
  };

  // Persistence step 1: write the flipped time slot and let the radio reboot.
  const runWriteTimeslot = async () => {
    const slot = Number(rtSlot);
    if (!port || !Number.isInteger(slot) || slot < 1) return;
    setBusy("writefield");
    setVerifyRead(null);
    // No custom error message — surface the real backend error.
    const res = await withToast(api.writeTimeslotAnytone(port, slot));
    setBusy("idle");
    if (!res) {
      await refresh();
      return;
    }
    setWriteField(res);
    // The radio is rebooting and will re-enumerate; refresh the port list.
    await refresh();
    const { toast } = await import("sonner");
    toast.success(
      `Wrote TS${res.new_value + 1} to slot ${res.slot}. Radio is rebooting — wait for it to boot, Rescan, reselect the port, then Verify.`,
    );
  };

  // Persistence step 2: fresh read to see whether the write stuck.
  const runVerifySlot = async () => {
    const slot = Number(rtSlot);
    if (!port || !Number.isInteger(slot) || slot < 1) return;
    setBusy("verifyslot");
    const res = await withToast(api.readSlotAnytone(port, slot));
    setBusy("idle");
    if (!res) {
      await refresh();
      return;
    }
    setVerifyRead(res);
    const { toast } = await import("sonner");
    const expected = writeField?.new_value;
    if (expected != null && res.time_slot_byte === expected) {
      toast.success(
        `Persisted! Slot ${res.slot} now reads TS${res.time_slot_byte + 1} — the write stuck across the reboot.`,
      );
    } else if (res.time_slot_byte != null) {
      toast.warning(
        `Slot ${res.slot} reads TS${res.time_slot_byte + 1}${
          expected != null ? ` (expected TS${expected + 1})` : ""
        }.`,
      );
    } else {
      toast.warning(`Slot ${res.slot} read back empty.`);
    }
  };

  // Persistence step 3: restore the original bytes from the backup.
  const runRestoreSlot = async () => {
    const slot = Number(rtSlot);
    if (!port || !writeField || !Number.isInteger(slot) || slot < 1) return;
    setBusy("restoreslot");
    const res = await withToast(
      api.restoreSlotAnytone(port, slot, writeField.backup_path),
    );
    setBusy("idle");
    if (!res) {
      await refresh();
      return;
    }
    setVerifyRead(null);
    await refresh();
    const { toast } = await import("sonner");
    toast.success(
      `Restored slot ${res.slot} from backup. Radio is rebooting — Rescan, then Verify to confirm.`,
    );
  };

  // Integrity investigation: capture a deterministic dump (keep the last two).
  const runDump = async () => {
    if (!port) return;
    setBusy("dump");
    setDumpDiff(null);
    const res = await withToast(api.dumpAnytoneRaw(port));
    setBusy("idle");
    if (!res) {
      await refresh();
      return;
    }
    setDumps((prev) => [...prev, res].slice(-2));
    // The radio reboots on exit; refresh the port list for the next capture.
    await refresh();
    const { toast } = await import("sonner");
    toast.success(
      `Captured ${res.total_bytes} bytes → ${res.path}. Make a one-field change in RT Systems, write to radio, then capture again and Diff.`,
    );
  };

  // Integrity investigation: diff the two most recent dumps.
  const runDiff = async () => {
    if (dumps.length < 2) return;
    setBusy("diff");
    const res = await withToast(
      api.diffAnytoneDumps(dumps[0].path, dumps[1].path),
    );
    setBusy("idle");
    if (!res) return;
    setDumpDiff(res);
    const { toast } = await import("sonner");
    toast.success(
      res.length === 0
        ? "No differences over the dumped regions."
        : `${res.length} differing byte-run(s) found.`,
    );
  };

  // Encoder write step 1: fresh read of the slot to prefill the edit form. The
  // encoder is a read-modify-write patch, so it needs a PROGRAMMED slot.
  const runEncLoad = async () => {
    const slot = Number(encSlot);
    if (!port || !Number.isInteger(slot) || slot < 1) return;
    setBusy("encload");
    setEncRead(null);
    setEncForm(null);
    setEncWriteRes(null);
    setEncVerify(null);
    const res = await withToast(api.readSlotAnytone(port, slot));
    setBusy("idle");
    if (!res) {
      await refresh();
      return;
    }
    setEncRead(res);
    const { toast } = await import("sonner");
    if (res.channel) {
      setEncForm(formFromChannel(res.channel));
      toast.success(`Loaded slot ${res.slot}: ${res.channel.name}`);
    } else {
      toast.warning(
        `Slot ${res.slot} is empty — the encoder patches an existing record, so pick a programmed slot.`,
      );
    }
  };

  // Encoder write step 2: batch write via the app-model encoder (whole-bank RMW,
  // backup-before-write, single END commit). The radio reboots afterwards.
  const runEncWrite = async () => {
    const slot = Number(encSlot);
    if (!port || !encForm || !Number.isInteger(slot) || slot < 1) return;
    const edit = editFromForm(encForm);
    const { toast } = await import("sonner");
    if (typeof edit === "string") {
      toast.error(edit);
      return;
    }
    setBusy("encwrite");
    setEncVerify(null);
    const res = await withToast(api.writeChannelsAnytone(port, [{ slot, edit }]));
    setBusy("idle");
    if (!res) {
      await refresh();
      return;
    }
    setEncWriteRes(res);
    // The radio is rebooting and will re-enumerate; refresh the port list.
    await refresh();
    toast.success(
      `Wrote slot ${slot} (${res.banks_written.length} bank(s)). Radio is rebooting — wait for it to boot, Rescan, reselect the port, then Verify.`,
    );
  };

  // Encoder write step 3: fresh-session read-back to confirm the edit stuck.
  const runEncVerify = async () => {
    const slot = Number(encSlot);
    if (!port || !Number.isInteger(slot) || slot < 1) return;
    setBusy("encverify");
    const res = await withToast(api.readSlotAnytone(port, slot));
    setBusy("idle");
    if (!res) {
      await refresh();
      return;
    }
    setEncVerify(res);
    const { toast } = await import("sonner");
    if (res.channel) {
      toast.success(
        `Fresh read of slot ${res.slot}: ${res.channel.name} — compare against what was written below.`,
      );
    } else {
      toast.warning(`Slot ${res.slot} read back empty.`);
    }
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

      {/* Stage 3: round-trip no-op write test. Reads one channel record, backs
          it up, writes the SAME bytes back, and reads again to confirm the write
          transport works WITHOUT changing the radio. Gated behind an explicit
          acknowledgement. */}
      <details className="rounded border border-amber-300 bg-amber-50/70 p-2 dark:border-amber-800/60 dark:bg-amber-950/30">
        <summary className="cursor-pointer text-[11px] font-medium text-amber-700 dark:text-amber-400">
          Write test — round-trip no-op (advanced)
        </summary>
        <div className="mt-2 space-y-2">
          <p className="text-[11px] text-slate-600 dark:text-slate-300">
            Validates the write path safely: reads one programmed channel slot,
            saves a backup, writes the <strong>same bytes</strong> back, then
            reads it again and confirms it is byte-for-byte identical. It does not
            change the radio's contents. Do this only with the radio connected and
            steady.
          </p>
          <div className="flex flex-wrap items-end gap-2">
            <label className="text-[11px] text-slate-600 dark:text-slate-300">
              Slot #
              <input
                type="number"
                min={1}
                value={rtSlot}
                onChange={(e) => setRtSlot(e.target.value)}
                className="ml-2 w-20 rounded border border-slate-300 bg-white px-1.5 py-0.5 text-xs dark:border-slate-600 dark:bg-slate-800"
              />
            </label>
            <label className="flex items-center gap-1.5 text-[11px] text-slate-600 dark:text-slate-300">
              <input
                type="checkbox"
                checked={rtAck}
                onChange={(e) => setRtAck(e.target.checked)}
              />
              I understand this writes to the radio
            </label>
            <Button
              variant="primary"
              onClick={runRoundtrip}
              disabled={!port || !rtAck || busy !== "idle"}
            >
              {busy === "roundtrip" ? (
                <Spinner className="h-3.5 w-3.5" />
              ) : null}
              Run round-trip
            </Button>
          </div>
          {roundtrip && (
            <div className="space-y-1 rounded border border-slate-200 bg-white/70 p-2 text-[11px] dark:border-slate-700 dark:bg-slate-900/40">
              <div
                className={
                  roundtrip.matched
                    ? "font-medium text-emerald-600 dark:text-emerald-400"
                    : "font-medium text-red-600 dark:text-red-400"
                }
              >
                {roundtrip.matched ? "✓ MATCH" : "✗ MISMATCH"} · slot{" "}
                {roundtrip.slot} @ {roundtrip.addr}
                {roundtrip.channel ? ` · ${roundtrip.channel.name}` : ""}
              </div>
              <div className="break-all text-slate-500 dark:text-slate-400">
                Backup: {roundtrip.backup_path}
              </div>
              <div className="break-all font-mono text-[10px] text-slate-400 dark:text-slate-500">
                before: {roundtrip.original_hex}
              </div>
              <div className="break-all font-mono text-[10px] text-slate-400 dark:text-slate-500">
                after:&nbsp; {roundtrip.readback_hex}
              </div>
            </div>
          )}
          {/* Persistence test: a real, committed write across a reboot. The
              D890UV only commits writes on leaving PC mode (it reboots), and
              in-session reads come from flash — so the only way to prove a write
              stuck is write → reboot → fresh read. Steps are separate because the
              radio re-enumerates across the reboot. */}
          <div className="space-y-2 rounded border border-slate-200 bg-white/70 p-2 dark:border-slate-700 dark:bg-slate-900/40">
            <p className="text-[11px] text-slate-600 dark:text-slate-300">
              <strong>Persistence test (DMR slot):</strong> writes a real change
              (flips the time slot), lets the radio reboot to commit, then reads
              it back in a fresh session. This{" "}
              <strong>leaves the slot changed</strong> until you Restore.
            </p>
            <div className="flex flex-wrap items-center gap-2">
              <Button
                variant="primary"
                onClick={runWriteTimeslot}
                disabled={!port || !rtAck || busy !== "idle"}
                title="DMR slot only: flips + commits the time slot, then the radio reboots"
              >
                {busy === "writefield" ? (
                  <Spinner className="h-3.5 w-3.5" />
                ) : null}
                1. Write TS flip
              </Button>
              <Button
                variant="ghost"
                onClick={runVerifySlot}
                disabled={!port || busy !== "idle"}
                title="Fresh read of the slot to see whether the change stuck"
              >
                {busy === "verifyslot" ? (
                  <Spinner className="h-3.5 w-3.5" />
                ) : null}
                2. Verify
              </Button>
              <Button
                variant="ghost"
                onClick={runRestoreSlot}
                disabled={!port || !rtAck || !writeField || busy !== "idle"}
                title="Write the backed-up original bytes back to the slot"
              >
                {busy === "restoreslot" ? (
                  <Spinner className="h-3.5 w-3.5" />
                ) : null}
                3. Restore
              </Button>
            </div>
            {writeField && (
              <div className="text-[11px] text-slate-500 dark:text-slate-400">
                Wrote {writeField.field} TS{writeField.old_value + 1} → TS
                {writeField.new_value + 1} on slot {writeField.slot} @{" "}
                {writeField.addr}
                {writeField.channel ? ` · ${writeField.channel.name}` : ""}.
                <span className="break-all"> Backup: {writeField.backup_path}</span>
              </div>
            )}
            {verifyRead && (
              <div
                className={
                  writeField &&
                  verifyRead.time_slot_byte === writeField.new_value
                    ? "text-[11px] font-medium text-emerald-600 dark:text-emerald-400"
                    : "text-[11px] font-medium text-amber-600 dark:text-amber-400"
                }
              >
                Fresh read: slot {verifyRead.slot} time slot ={" "}
                {verifyRead.time_slot_byte != null
                  ? `TS${verifyRead.time_slot_byte + 1}`
                  : "(empty)"}
                {writeField
                  ? verifyRead.time_slot_byte === writeField.new_value
                    ? " ✓ write persisted"
                    : ` (expected TS${writeField.new_value + 1})`
                  : ""}
              </div>
            )}
          </div>

          {/* Integrity investigation (read-only): find the checksum/valid region
              a per-record write corrupts. Capture a dump, change ONE field in RT
              Systems + write to radio, capture again, then Diff — the change
              beyond the channel record is the integrity structure. */}
          <div className="space-y-2 rounded border border-slate-200 bg-white/70 p-2 dark:border-slate-700 dark:bg-slate-900/40">
            <p className="text-[11px] text-slate-600 dark:text-slate-300">
              <strong>Integrity investigation (read-only):</strong> Capture a
              dump → change ONE field in RT Systems and write to the radio →
              Capture again → Diff. Any change <em>beyond</em> the channel record
              is the checksum/valid structure our writes must maintain.
            </p>
            <div className="flex flex-wrap items-center gap-2">
              <Button
                variant="ghost"
                onClick={runDump}
                disabled={!port || busy !== "idle"}
              >
                {busy === "dump" ? <Spinner className="h-3.5 w-3.5" /> : null}
                Capture dump ({dumps.length}/2)
              </Button>
              <Button
                variant="ghost"
                onClick={runDiff}
                disabled={dumps.length < 2 || busy !== "idle"}
              >
                {busy === "diff" ? <Spinner className="h-3.5 w-3.5" /> : null}
                Diff last two
              </Button>
            </div>
            {dumps.length > 0 && (
              <div className="space-y-0.5 text-[10px] text-slate-500 dark:text-slate-400">
                {dumps.map((d, i) => (
                  <div key={d.path} className="break-all">
                    {i === 0 && dumps.length === 2 ? "A: " : i === 1 || dumps.length === 1 ? "B: " : "A: "}
                    {d.path} ({d.total_bytes}B)
                  </div>
                ))}
              </div>
            )}
            {dumpDiff && (
              <div className="max-h-56 space-y-0.5 overflow-auto rounded border border-slate-200 bg-white/70 p-1.5 font-mono text-[10px] dark:border-slate-700 dark:bg-slate-900/40">
                {dumpDiff.length === 0 ? (
                  <div className="text-slate-500 dark:text-slate-400">
                    No differences over the dumped regions.
                  </div>
                ) : (
                  dumpDiff.map((d) => (
                    <div key={d.addr} className="break-all">
                      <span className="text-amber-600 dark:text-amber-400">
                        {d.addr}
                      </span>{" "}
                      <span className="text-slate-400">({d.len}B)</span>{" "}
                      <span className="text-red-500">{d.a_hex}</span>
                      {" → "}
                      <span className="text-emerald-600 dark:text-emerald-400">
                        {d.b_hex}
                      </span>
                    </div>
                  ))
                )}
              </div>
            )}
          </div>
        </div>
      </details>

      {/* Stage 3 encoder write: the first full app-model→record write path. Load
          a programmed slot (prefills the form from the radio's own bytes), edit
          any modeled field, Write (whole-bank RMW + backup-before-write + single
          commit/reboot), then Verify with a fresh read after re-enumeration. */}
      <details className="rounded border border-amber-300 bg-amber-50/70 p-2 dark:border-amber-800/60 dark:bg-amber-950/30">
        <summary className="cursor-pointer text-[11px] font-medium text-amber-700 dark:text-amber-400">
          Channel edit — full encoder write (advanced)
        </summary>
        <div className="mt-2 space-y-2">
          <p className="text-[11px] text-slate-600 dark:text-slate-300">
            Writes a real channel edit through the app-model encoder: Load a
            programmed slot to prefill the form from the radio, change fields,
            then Write. Only the modeled fields are patched (whole-bank
            read-modify-write); every affected bank is backed up before any byte
            is written, and the radio commits + reboots once. Verify with a fresh
            read after it re-enumerates.
          </p>
          <div className="flex flex-wrap items-end gap-2">
            <label className="text-[11px] text-slate-600 dark:text-slate-300">
              Slot #
              <input
                type="number"
                min={1}
                value={encSlot}
                onChange={(e) => setEncSlot(e.target.value)}
                className="ml-2 w-20 rounded border border-slate-300 bg-white px-1.5 py-0.5 text-xs dark:border-slate-600 dark:bg-slate-800"
              />
            </label>
            <Button
              variant="ghost"
              onClick={runEncLoad}
              disabled={!port || busy !== "idle"}
            >
              {busy === "encload" ? <Spinner className="h-3.5 w-3.5" /> : null}
              1. Load slot
            </Button>
            <label className="flex items-center gap-1.5 text-[11px] text-slate-600 dark:text-slate-300">
              <input
                type="checkbox"
                checked={encAck}
                onChange={(e) => setEncAck(e.target.checked)}
              />
              I understand this writes to the radio
            </label>
          </div>
          {encForm && (
            <div className="space-y-2 rounded border border-slate-200 bg-white/70 p-2 dark:border-slate-700 dark:bg-slate-900/40">
              <div className="grid grid-cols-2 gap-2 sm:grid-cols-3">
                <label className="text-[11px] text-slate-600 dark:text-slate-300">
                  Name
                  <input
                    type="text"
                    maxLength={16}
                    value={encForm.name}
                    onChange={(e) =>
                      setEncForm({ ...encForm, name: e.target.value })
                    }
                    className="mt-0.5 block w-full rounded border border-slate-300 bg-white px-1.5 py-0.5 text-xs dark:border-slate-600 dark:bg-slate-800"
                  />
                </label>
                <label className="text-[11px] text-slate-600 dark:text-slate-300">
                  RX (MHz)
                  <input
                    type="text"
                    value={encForm.rx}
                    onChange={(e) =>
                      setEncForm({ ...encForm, rx: e.target.value })
                    }
                    className="mt-0.5 block w-full rounded border border-slate-300 bg-white px-1.5 py-0.5 font-mono text-xs dark:border-slate-600 dark:bg-slate-800"
                  />
                </label>
                <label className="text-[11px] text-slate-600 dark:text-slate-300">
                  TX (MHz)
                  <input
                    type="text"
                    value={encForm.tx}
                    onChange={(e) =>
                      setEncForm({ ...encForm, tx: e.target.value })
                    }
                    className="mt-0.5 block w-full rounded border border-slate-300 bg-white px-1.5 py-0.5 font-mono text-xs dark:border-slate-600 dark:bg-slate-800"
                  />
                </label>
                <label className="text-[11px] text-slate-600 dark:text-slate-300">
                  Power
                  <Select
                    className="mt-0.5 w-full"
                    value={encForm.power}
                    onChange={(e) =>
                      setEncForm({ ...encForm, power: e.target.value })
                    }
                  >
                    {ANYTONE_POWER.map((p, i) => (
                      <option key={p} value={String(i)}>
                        {p}
                      </option>
                    ))}
                  </Select>
                </label>
                <label className="text-[11px] text-slate-600 dark:text-slate-300">
                  Bandwidth
                  <Select
                    className="mt-0.5 w-full"
                    value={encForm.wide ? "1" : "0"}
                    onChange={(e) =>
                      setEncForm({ ...encForm, wide: e.target.value === "1" })
                    }
                  >
                    <option value="0">12.5 kHz</option>
                    <option value="1">25 kHz</option>
                  </Select>
                </label>
                <label className="text-[11px] text-slate-600 dark:text-slate-300">
                  Mode
                  <Select
                    className="mt-0.5 w-full"
                    value={encForm.mode}
                    onChange={(e) =>
                      setEncForm({ ...encForm, mode: e.target.value })
                    }
                  >
                    {ANYTONE_MODES.map((m, i) => (
                      <option key={m} value={String(i)}>
                        {m}
                      </option>
                    ))}
                  </Select>
                </label>
                {Number(encForm.mode) === 0 ? (
                  <>
                    <label className="text-[11px] text-slate-600 dark:text-slate-300">
                      TX tone
                      <div className="mt-0.5 flex gap-1">
                        <Select
                          className="w-20"
                          value={encForm.toneTxKind}
                          onChange={(e) =>
                            setEncForm({
                              ...encForm,
                              toneTxKind: e.target.value as ToneKind,
                            })
                          }
                        >
                          <option value="none">None</option>
                          <option value="ctcss">CTCSS</option>
                          <option value="dcs">DCS</option>
                        </Select>
                        <input
                          type="text"
                          value={encForm.toneTxVal}
                          disabled={encForm.toneTxKind === "none"}
                          onChange={(e) =>
                            setEncForm({ ...encForm, toneTxVal: e.target.value })
                          }
                          className="w-16 rounded border border-slate-300 bg-white px-1.5 py-0.5 font-mono text-xs disabled:opacity-40 dark:border-slate-600 dark:bg-slate-800"
                        />
                      </div>
                    </label>
                    <label className="text-[11px] text-slate-600 dark:text-slate-300">
                      RX tone
                      <div className="mt-0.5 flex gap-1">
                        <Select
                          className="w-20"
                          value={encForm.toneRxKind}
                          onChange={(e) =>
                            setEncForm({
                              ...encForm,
                              toneRxKind: e.target.value as ToneKind,
                            })
                          }
                        >
                          <option value="none">None</option>
                          <option value="ctcss">CTCSS</option>
                          <option value="dcs">DCS</option>
                        </Select>
                        <input
                          type="text"
                          value={encForm.toneRxVal}
                          disabled={encForm.toneRxKind === "none"}
                          onChange={(e) =>
                            setEncForm({ ...encForm, toneRxVal: e.target.value })
                          }
                          className="w-16 rounded border border-slate-300 bg-white px-1.5 py-0.5 font-mono text-xs disabled:opacity-40 dark:border-slate-600 dark:bg-slate-800"
                        />
                      </div>
                    </label>
                  </>
                ) : (
                  <>
                    <label className="text-[11px] text-slate-600 dark:text-slate-300">
                      Color code
                      <input
                        type="number"
                        min={0}
                        max={15}
                        value={encForm.colorCode}
                        onChange={(e) =>
                          setEncForm({ ...encForm, colorCode: e.target.value })
                        }
                        className="mt-0.5 block w-full rounded border border-slate-300 bg-white px-1.5 py-0.5 text-xs dark:border-slate-600 dark:bg-slate-800"
                      />
                    </label>
                    <label className="text-[11px] text-slate-600 dark:text-slate-300">
                      Time slot
                      <Select
                        className="mt-0.5 w-full"
                        value={encForm.timeSlot}
                        onChange={(e) =>
                          setEncForm({ ...encForm, timeSlot: e.target.value })
                        }
                      >
                        <option value="1">TS1</option>
                        <option value="2">TS2</option>
                      </Select>
                    </label>
                    <label className="text-[11px] text-slate-600 dark:text-slate-300">
                      Contact #
                      <input
                        type="number"
                        min={0}
                        value={encForm.contactIndex}
                        onChange={(e) =>
                          setEncForm({
                            ...encForm,
                            contactIndex: e.target.value,
                          })
                        }
                        className="mt-0.5 block w-full rounded border border-slate-300 bg-white px-1.5 py-0.5 text-xs dark:border-slate-600 dark:bg-slate-800"
                      />
                    </label>
                  </>
                )}
              </div>
              <div className="flex flex-wrap items-center gap-2">
                <Button
                  variant="primary"
                  onClick={runEncWrite}
                  disabled={
                    !port || !encAck || !encRead?.channel || busy !== "idle"
                  }
                  title="Whole-bank RMW write of this edit; backs up first, then commits (radio reboots)"
                >
                  {busy === "encwrite" ? (
                    <Spinner className="h-3.5 w-3.5" />
                  ) : null}
                  2. Write channel
                </Button>
                <Button
                  variant="ghost"
                  onClick={runEncVerify}
                  disabled={!port || busy !== "idle"}
                  title="Fresh read of the slot after the reboot to confirm the edit stuck"
                >
                  {busy === "encverify" ? (
                    <Spinner className="h-3.5 w-3.5" />
                  ) : null}
                  3. Verify
                </Button>
              </div>
            </div>
          )}
          {encWriteRes && (
            <div className="space-y-1 rounded border border-slate-200 bg-white/70 p-2 text-[11px] dark:border-slate-700 dark:bg-slate-900/40">
              <div className="text-slate-600 dark:text-slate-300">
                Wrote slot(s) {encWriteRes.slots.join(", ")} · bank(s){" "}
                {encWriteRes.banks_written.join(", ")}
              </div>
              <div className="break-all text-slate-500 dark:text-slate-400">
                Backup: {encWriteRes.backup_path}
              </div>
              <div className="text-slate-500 dark:text-slate-400">
                {encWriteRes.note}
              </div>
            </div>
          )}
          {encVerify && encForm && (
            <div className="space-y-1 rounded border border-slate-200 bg-white/70 p-2 text-[11px] dark:border-slate-700 dark:bg-slate-900/40">
              <div className="text-slate-500 dark:text-slate-400">
                Wrote:{" "}
                <span className="font-mono text-[10px]">
                  {formSummary(encForm)}
                </span>
              </div>
              <div
                className={
                  encVerify.channel
                    ? "text-emerald-600 dark:text-emerald-400"
                    : "text-amber-600 dark:text-amber-400"
                }
              >
                Read:&nbsp;{" "}
                <span className="font-mono text-[10px]">
                  {encVerify.channel
                    ? channelSummary(encVerify.channel)
                    : "(slot read back empty)"}
                </span>
              </div>
              <div className="break-all font-mono text-[10px] text-slate-400 dark:text-slate-500">
                {encVerify.hex}
              </div>
            </div>
          )}
        </div>
      </details>
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
        <SpecRow
          label="Frequency range"
          value={
            model.freq_min != null && model.freq_max != null
              ? `${model.freq_min}–${model.freq_max} MHz`
              : "—"
          }
        />
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

function SettingsField({
  field,
  value,
  onChange,
}: {
  field: SettingField;
  value: string | number | boolean;
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

  // Form state, re-seeded whenever a different profile is selected.
  const [name, setName] = useState(profile.display_name);
  const [notes, setNotes] = useState(profile.notes ?? "");
  const [values, setValues] = useState<SettingsValues>({});
  const [lastId, setLastId] = useState<number | null>(null);
  if (profile.id !== lastId) {
    setName(profile.display_name);
    setNotes(profile.notes ?? "");
    setValues(seedValues(fields, parseSettings(profile.non_channel_settings)));
    setLastId(profile.id);
    setTab("settings");
  }

  const setValue = (key: string, v: string | number | boolean) =>
    setValues((s) => ({ ...s, [key]: v }));

  const save = async () => {
    if (!name.trim()) return;
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
    if (updated) onSaved(updated);
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
            {(model.model === "UV-5R" ||
              model.model === "TD-H3" ||
              model.model === "AT-D890UV") &&
              fields.length > 0 && (
                <RadioSyncBar
                  profileId={profile.id}
                  modelLabel={model.display_name}
                  read={
                    model.model === "TD-H3"
                      ? api.readTdh3SettingsForProfile
                      : model.model === "AT-D890UV"
                        ? api.readAnytoneSettingsForProfile
                        : api.readRadioSettings
                  }
                  onLoaded={(s) => setValues((v) => ({ ...v, ...s }))}
                />
              )}
            {model.model === "AT-D890UV" && (
              <AnytoneBackupBar modelLabel={model.display_name} />
            )}
            {fields.length === 0 ? (
              <p className="text-xs text-slate-400">
                This model has no configurable non-channel settings.
              </p>
            ) : (
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
                      onChange={(v) => setValue(f.key, v)}
                    />
                  ),
                )}
              </div>
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
