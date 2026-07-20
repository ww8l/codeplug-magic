import { useEffect, useMemo, useState, type ReactNode } from "react";
import { confirm as confirmDialog } from "@tauri-apps/plugin-dialog";
import { Trash2, Save, DownloadCloud, RefreshCw } from "lucide-react";
import clsx from "clsx";
import { api, withToast } from "../../lib/api";
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
