import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import clsx from "clsx";
import { confirm as confirmDialog } from "@tauri-apps/plugin-dialog";
import { Check, Trash2, Plus, X } from "lucide-react";
import { api, withToast } from "../../lib/api";
import type {
  Channel,
  ChannelInput,
  CityCentroid,
  RepeaterTalkgroup,
  Talkgroup,
} from "../../lib/types";
import {
  CROSS_MODES,
  CTCSS_TONES,
  DTCS_CODES,
  DTCS_POLARITIES,
  DUPLEX_OPTIONS,
  MODES,
  OP_STATUSES,
  POWER_LEVELS,
  SERVICE_TYPES,
  TIMESLOTS,
  TONE_MODES,
  USE_TYPES,
  fmtFreq,
  fmtOffset,
} from "../../lib/constants";
import { SlideOver } from "../overlays";
import { Button, Select, TextInput, Badge } from "../ui";

function blankInput(): ChannelInput {
  return {
    name_long: "",
    name_short: "",
    callsign: "",
    rx_freq: 0,
    tx_freq: null,
    offset: null,
    duplex: null,
    mode: "FM",
    tone_mode: "off",
    ctcss_uplink: null,
    ctcss_downlink: null,
    dcs_code: null,
    dcs_rx_code: null,
    dcs_polarity: "NN",
    cross_mode: "Tone->Tone",
    power: null,
    dmr_color_code: null,
    dmr_timeslot: null,
    dmr_talkgroup: null,
    dstar_capable: false,
    ysf_capable: false,
    nxdn_capable: false,
    p25_capable: false,
    p25_nac: null,
    m17_capable: false,
    m17_can: null,
    use_type: null,
    operational_status: "Unknown",
    service_type: "Amateur",
    city: "",
    county: null,
    state: "",
    country: "United States",
    latitude: null,
    longitude: null,
    notes: null,
    source: "manual",
  };
}

function toInput(c: Channel): ChannelInput {
  return {
    name_long: c.name_long,
    name_short: c.name_short,
    callsign: c.callsign,
    rx_freq: c.rx_freq,
    tx_freq: c.tx_freq,
    offset: c.offset,
    duplex: c.duplex,
    mode: c.mode,
    tone_mode: c.tone_mode,
    ctcss_uplink: c.ctcss_uplink,
    ctcss_downlink: c.ctcss_downlink,
    dcs_code: c.dcs_code,
    dcs_rx_code: c.dcs_rx_code,
    dcs_polarity: c.dcs_polarity ?? "NN",
    cross_mode: c.cross_mode ?? "Tone->Tone",
    power: c.power,
    dmr_color_code: c.dmr_color_code,
    dmr_timeslot: c.dmr_timeslot,
    dmr_talkgroup: c.dmr_talkgroup,
    dstar_capable: c.dstar_capable,
    ysf_capable: c.ysf_capable,
    nxdn_capable: c.nxdn_capable,
    p25_capable: c.p25_capable,
    p25_nac: c.p25_nac,
    m17_capable: c.m17_capable,
    m17_can: c.m17_can,
    use_type: c.use_type,
    operational_status: c.operational_status,
    service_type: c.service_type,
    city: c.city,
    county: c.county,
    state: c.state,
    country: c.country,
    latitude: c.latitude,
    longitude: c.longitude,
    notes: c.notes,
    source: c.source,
  };
}

function Field({ label, children }: { label: string; children: ReactNode }) {
  return (
    <label className="flex flex-col gap-1">
      <span className="text-[11px] font-medium text-slate-500 dark:text-slate-400">
        {label}
      </span>
      {children}
    </label>
  );
}

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <div className="space-y-2">
      <h3 className="text-[11px] font-semibold uppercase tracking-wide text-slate-400">
        {title}
      </h3>
      <div className="grid grid-cols-2 gap-3">{children}</div>
    </div>
  );
}

// A small "RB says: X / Accept" affordance shown under an overridable field.
function RbOverride({
  show,
  rbValue,
  onAccept,
}: {
  show: boolean;
  rbValue: string;
  onAccept: () => void;
}) {
  if (!show) return null;
  return (
    <div className="mt-1 flex items-center justify-between rounded bg-amber-50 px-1.5 py-1 text-[11px] text-amber-700 dark:bg-amber-950/40 dark:text-amber-400">
      <span>RB says: {rbValue}</span>
      <button
        onClick={onAccept}
        className="inline-flex items-center gap-0.5 font-medium hover:underline"
      >
        <Check size={11} /> Accept RB value
      </button>
    </div>
  );
}

/** CTCSS frequency picker. Includes the current value even if non-standard. */
function CtcssSelect({
  value,
  onChange,
}: {
  value: number | null;
  onChange: (v: number | null) => void;
}) {
  const options =
    value != null && !CTCSS_TONES.includes(value)
      ? [...CTCSS_TONES, value].sort((a, b) => a - b)
      : CTCSS_TONES;
  return (
    <Select
      value={value != null ? String(value) : ""}
      onChange={(e) => onChange(e.target.value === "" ? null : Number(e.target.value))}
    >
      <option value="">— none —</option>
      {options.map((t) => (
        <option key={t} value={t}>
          {t.toFixed(1)}
        </option>
      ))}
    </Select>
  );
}

/** DTCS code picker. Includes the current value even if non-standard. */
function DtcsSelect({
  value,
  onChange,
}: {
  value: string | null;
  onChange: (v: string | null) => void;
}) {
  const options =
    value && !DTCS_CODES.includes(value) ? [value, ...DTCS_CODES] : DTCS_CODES;
  return (
    <Select value={value ?? ""} onChange={(e) => onChange(e.target.value || null)}>
      <option value="">— none —</option>
      {options.map((c) => (
        <option key={c} value={c}>
          {c}
        </option>
      ))}
    </Select>
  );
}

export function ChannelDetailPanel({
  channel,
  mode,
  open,
  onClose,
  onSaved,
  sourceSuggestions = [],
  cities = [],
}: {
  channel: Channel | null;
  mode: "edit" | "create";
  open: boolean;
  onClose: () => void;
  onSaved: () => void;
  /** Existing distinct `source` values, offered as autocomplete suggestions. */
  sourceSuggestions?: string[];
  /** City centroids from the local data, used to auto-fill coordinates. */
  cities?: CityCentroid[];
}) {
  const [form, setForm] = useState<ChannelInput>(blankInput());
  const [saving, setSaving] = useState(false);
  // Raw text for the frequency fields, kept separate from the parsed numbers so
  // an in-progress entry like "146." (or "146.9") survives a re-render — parsing
  // to a number on each keystroke would drop the trailing dot and block typing.
  const [rxText, setRxText] = useState("");
  const [txText, setTxText] = useState("");
  // Coordinates we last auto-filled, so the geocode effect can tell "the user
  // hasn't touched these" from "the user typed their own" and avoid clobbering.
  const autoGeoRef = useRef<{ lat: number; lon: number } | null>(null);
  const [geoStatus, setGeoStatus] = useState<
    "idle" | "looking" | "data" | "online" | "notfound" | "error"
  >("idle");

  // Re-seed the form whenever the target channel changes. For "create" the
  // seedKey is the constant "new", so we also clear lastKey on close — that way
  // reopening to add another repeater always re-seeds to a blank form instead of
  // reusing the values from the channel just added.
  const seedKey = mode === "edit" ? channel?.id : "new";
  const [lastKey, setLastKey] = useState<number | string | undefined>(undefined);
  if (open && seedKey !== lastKey) {
    const seeded = mode === "edit" && channel ? toInput(channel) : blankInput();
    setForm(seeded);
    setRxText(seeded.rx_freq ? String(seeded.rx_freq) : "");
    setTxText(seeded.tx_freq != null ? String(seeded.tx_freq) : "");
    setLastKey(seedKey);
    autoGeoRef.current = null;
    setGeoStatus("idle");
  } else if (!open && lastKey !== undefined) {
    setLastKey(undefined);
  }

  const set = <K extends keyof ChannelInput>(key: K, value: ChannelInput[K]) =>
    setForm((f) => ({ ...f, [key]: value }));

  // Auto-fill lat/lon from the city. In "create" it fires as the user types a
  // city; in "edit" it backfills a channel that has a city but no coordinates
  // (e.g. RMHAM/RepeaterBook imports that lack lat/lon). Prefers a centroid from
  // the user's own data (instant, offline), else the online geocoder. The
  // "untouched" guard means it only fills EMPTY coords (or ones we filled) — it
  // never overwrites coordinates the channel already has or the user typed.
  useEffect(() => {
    if (!open) return;
    const city = (form.city ?? "").trim();
    if (!city) {
      setGeoStatus("idle");
      return;
    }
    const auto = autoGeoRef.current;
    const untouched =
      (form.latitude == null && form.longitude == null) ||
      (auto != null &&
        form.latitude === auto.lat &&
        form.longitude === auto.lon);
    if (!untouched) return;

    const st = (form.state ?? "").trim();
    const country = (form.country ?? "").trim();
    let cancelled = false;

    const apply = (lat: number, lon: number, via: "data" | "online") => {
      autoGeoRef.current = { lat, lon };
      setForm((f) => ({ ...f, latitude: lat, longitude: lon }));
      setGeoStatus(via);
    };

    const timer = setTimeout(async () => {
      // 1) Match a town already in the local data (same source the distance
      //    search uses). When the state is blank and a name repeats across
      //    states, take the one backed by the most repeaters.
      const wantCity = city.toLowerCase();
      const wantState = st.toLowerCase();
      const hit = cities
        .filter(
          (c) =>
            c.city.toLowerCase() === wantCity &&
            (wantState === "" || (c.state ?? "").toLowerCase() === wantState),
        )
        .sort((a, b) => b.count - a.count)[0];
      if (hit) {
        if (!cancelled) apply(hit.latitude, hit.longitude, "data");
        return;
      }
      // 2) Fall back to the online geocoder.
      if (!cancelled) setGeoStatus("looking");
      try {
        const pt = await api.geocodeCity(city, st || null, country || null);
        if (cancelled) return;
        if (pt) apply(pt.latitude, pt.longitude, "online");
        else setGeoStatus("notfound");
      } catch {
        if (!cancelled) setGeoStatus("error");
      }
    }, 700);

    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
    // Intentionally omit form.latitude/longitude: they are read as a guard only,
    // and including them would re-fire the effect on our own fill (a loop).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mode, open, form.city, form.state, form.country, cities]);

  const num = (v: string): number | null => {
    if (v.trim() === "") return null;
    const n = Number(v);
    return Number.isNaN(n) ? null : n;
  };

  const save = async () => {
    if (!form.rx_freq || form.rx_freq <= 0) {
      const { toast } = await import("sonner");
      toast.error("RX frequency is required.");
      return;
    }
    setSaving(true);
    const res =
      mode === "edit" && channel
        ? await withToast(api.updateChannel(channel.id, form), {
            success: "Channel updated",
          })
        : await withToast(api.createChannel(form), { success: "Channel added" });
    setSaving(false);
    if (res) {
      onSaved();
      onClose();
    }
  };

  const acceptRb = async (field: string) => {
    if (!channel) return;
    const updated = await withToast(api.acceptRbValue(channel.id, field), {
      success: "Reverted to RepeaterBook value",
    });
    if (updated) {
      setForm(toInput(updated));
      onSaved();
    }
  };

  const remove = async () => {
    if (!channel) return;
    const label = channel.name_long || channel.name_short || channel.callsign || "this repeater";
    const ok = await confirmDialog(`Delete ${label}? This cannot be undone.`, {
      title: "Delete repeater",
      kind: "warning",
    });
    if (!ok) return;
    const res = await withToast(api.deleteChannel(channel.id), {
      success: "Repeater deleted",
    });
    if (res !== undefined) {
      onSaved();
      onClose();
    }
  };

  const isRb = channel?.source === "repeaterbook";

  // Tone-mode driven field visibility (CHIRP universal scheme). For Cross, the
  // two sides come from cross_mode "TX->RX"; otherwise each mode implies them.
  const tmode = form.tone_mode ?? "off";
  let txSide = "";
  let rxSide = "";
  if (tmode === "Tone") {
    txSide = "Tone";
  } else if (tmode === "TSQL") {
    txSide = rxSide = "Tone";
  } else if (tmode === "DTCS") {
    txSide = rxSide = "DTCS";
  } else if (tmode === "Cross") {
    [txSide, rxSide] = (form.cross_mode || "Tone->Tone").split("->");
  }

  return (
    <SlideOver
      open={open}
      onClose={onClose}
      title={
        mode === "create"
          ? "Add Channel"
          : channel?.name_long || channel?.name_short || "Channel"
      }
      subtitle={
        mode === "edit" && channel
          ? `${fmtFreq(channel.rx_freq)} MHz · ${channel.band ?? ""}`
          : "New manual channel"
      }
      footer={
        <>
          {mode === "edit" && channel && (
            <Button variant="danger" className="mr-auto" onClick={remove}>
              <Trash2 size={14} /> Delete
            </Button>
          )}
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" onClick={save} disabled={saving}>
            {mode === "create" ? "Add Channel" : "Save Changes"}
          </Button>
        </>
      }
    >
      <div className="space-y-5">
        {mode === "edit" && channel && (
          <div className="flex flex-wrap items-center gap-2 rounded-md bg-slate-50 p-2.5 text-xs dark:bg-slate-900/60">
            <Badge
              className={
                isRb
                  ? "bg-sky-100 text-sky-700 dark:bg-sky-950 dark:text-sky-300"
                  : undefined
              }
            >
              {channel.source}
            </Badge>
            {channel.rb_name && (
              <span className="text-slate-500 dark:text-slate-400">
                RepeaterBook Name:{" "}
                <span className="font-medium text-slate-700 dark:text-slate-200">
                  {channel.rb_name}
                </span>
              </span>
            )}
          </div>
        )}

        <Section title="Identity">
          <Field label="Name (long, ≤16)">
            <TextInput
              maxLength={16}
              value={form.name_long ?? ""}
              placeholder="W0CPH Colorado Spgs"
              onChange={(e) => set("name_long", e.target.value)}
            />
          </Field>
          <Field label="Name (short, ≤7)">
            <TextInput
              maxLength={7}
              value={form.name_short ?? ""}
              placeholder="W0CPH"
              onChange={(e) => set("name_short", e.target.value)}
            />
          </Field>
          <Field label="Callsign">
            <TextInput
              value={form.callsign ?? ""}
              placeholder="W0CPH"
              onChange={(e) => set("callsign", e.target.value)}
            />
          </Field>
          <Field label="Source">
            <TextInput
              list="channel-source-suggestions"
              value={form.source ?? ""}
              placeholder="manual"
              onChange={(e) => set("source", e.target.value)}
            />
            <datalist id="channel-source-suggestions">
              {sourceSuggestions.map((s) => (
                <option key={s} value={s} />
              ))}
            </datalist>
          </Field>
        </Section>

        <Section title="Frequency">
          <Field label="RX Freq (MHz) *">
            <TextInput
              value={rxText}
              inputMode="decimal"
              placeholder="146.940"
              onChange={(e) => {
                setRxText(e.target.value);
                set("rx_freq", num(e.target.value) ?? 0);
              }}
            />
          </Field>
          <Field label="TX Freq (MHz)">
            <TextInput
              value={txText}
              inputMode="decimal"
              placeholder="146.340"
              onChange={(e) => {
                setTxText(e.target.value);
                set("tx_freq", num(e.target.value));
              }}
            />
          </Field>
          <Field label="Duplex (derived)">
            <Select value={form.duplex ?? ""} onChange={(e) => set("duplex", e.target.value || null)}>
              <option value="">auto</option>
              {DUPLEX_OPTIONS.map((d) => (
                <option key={d} value={d}>
                  {d}
                </option>
              ))}
            </Select>
          </Field>
          <Field label="Offset (derived)">
            <TextInput disabled value={fmtOffset(form.offset)} placeholder="auto from RX/TX" />
          </Field>
        </Section>

        <Section title="Tone & Power">
          <Field label="Tone Mode">
            <Select value={form.tone_mode ?? "off"} onChange={(e) => set("tone_mode", e.target.value)}>
              {TONE_MODES.map((t) => (
                <option key={t} value={t}>
                  {t}
                </option>
              ))}
            </Select>
          </Field>
          <Field label="TX Power">
            <Select value={form.power ?? ""} onChange={(e) => set("power", e.target.value || null)}>
              <option value="">Radio max</option>
              {POWER_LEVELS.map((p) => (
                <option key={p} value={p}>
                  {p}
                </option>
              ))}
            </Select>
          </Field>

          {tmode === "Cross" && (
            <Field label="Cross Mode">
              <Select value={form.cross_mode} onChange={(e) => set("cross_mode", e.target.value)}>
                {CROSS_MODES.map((m) => (
                  <option key={m} value={m}>
                    {m}
                  </option>
                ))}
              </Select>
            </Field>
          )}
          {txSide === "Tone" && tmode !== "TSQL" && (
            <Field label="TX Tone (CTCSS, Hz)">
              <CtcssSelect value={form.ctcss_uplink} onChange={(v) => set("ctcss_uplink", v)} />
              <RbOverride
                show={!!channel?.ctcss_uplink_overridden && channel?.rb_ctcss_uplink != null}
                rbValue={channel?.rb_ctcss_uplink?.toFixed(1) ?? ""}
                onAccept={() => acceptRb("ctcss_uplink")}
              />
            </Field>
          )}
          {(rxSide === "Tone" || tmode === "TSQL") && (
            <Field label={tmode === "TSQL" ? "Tone (CTCSS, Hz)" : "RX Tone (CTCSS, Hz)"}>
              <CtcssSelect value={form.ctcss_downlink} onChange={(v) => set("ctcss_downlink", v)} />
              <RbOverride
                show={!!channel?.ctcss_downlink_overridden && channel?.rb_ctcss_downlink != null}
                rbValue={channel?.rb_ctcss_downlink?.toFixed(1) ?? ""}
                onAccept={() => acceptRb("ctcss_downlink")}
              />
            </Field>
          )}
          {txSide === "DTCS" && (
            <Field label={tmode === "DTCS" ? "DTCS Code" : "TX DTCS Code"}>
              <DtcsSelect value={form.dcs_code} onChange={(v) => set("dcs_code", v)} />
            </Field>
          )}
          {rxSide === "DTCS" && tmode === "Cross" && (
            <Field label="RX DTCS Code">
              <DtcsSelect value={form.dcs_rx_code} onChange={(v) => set("dcs_rx_code", v)} />
            </Field>
          )}
          {(txSide === "DTCS" || rxSide === "DTCS") && (
            <Field label="DTCS Polarity (TX/RX)">
              <Select value={form.dcs_polarity} onChange={(e) => set("dcs_polarity", e.target.value)}>
                {DTCS_POLARITIES.map((p) => (
                  <option key={p} value={p}>
                    {p}
                  </option>
                ))}
              </Select>
            </Field>
          )}
        </Section>

        <Section title="Location">
          <Field label="City">
            <TextInput value={form.city ?? ""} onChange={(e) => set("city", e.target.value)} />
          </Field>
          <Field label="County">
            <TextInput value={form.county ?? ""} onChange={(e) => set("county", e.target.value || null)} />
          </Field>
          <Field label="State">
            <TextInput value={form.state ?? ""} onChange={(e) => set("state", e.target.value)} />
          </Field>
          <Field label="Country">
            <TextInput value={form.country ?? ""} onChange={(e) => set("country", e.target.value || null)} />
          </Field>
          <Field label="Latitude">
            <TextInput
              value={form.latitude != null ? String(form.latitude) : ""}
              onChange={(e) => {
                set("latitude", num(e.target.value));
                setGeoStatus("idle");
              }}
            />
          </Field>
          <Field label="Longitude">
            <TextInput
              value={form.longitude != null ? String(form.longitude) : ""}
              onChange={(e) => {
                set("longitude", num(e.target.value));
                setGeoStatus("idle");
              }}
            />
          </Field>
          {geoStatus !== "idle" && (
            <p
              className={clsx(
                "col-span-2 text-[11px]",
                (geoStatus === "data" || geoStatus === "online") &&
                  "text-emerald-600 dark:text-emerald-400",
                (geoStatus === "notfound" || geoStatus === "error") &&
                  "text-amber-600 dark:text-amber-400",
                geoStatus === "looking" && "text-slate-400",
              )}
            >
              {geoStatus === "looking" &&
                `Looking up coordinates for ${(form.city ?? "").trim()}…`}
              {geoStatus === "data" &&
                "Coordinates filled from your existing repeaters in this town."}
              {geoStatus === "online" &&
                "Coordinates filled from OpenStreetMap. Adjust if needed."}
              {geoStatus === "notfound" &&
                "Couldn't find that town — enter coordinates manually."}
              {geoStatus === "error" &&
                "Geocoder unavailable — enter coordinates manually."}
            </p>
          )}
        </Section>

        <Section title="Classification">
          <Field label="Use">
            <Select value={form.use_type ?? ""} onChange={(e) => set("use_type", e.target.value || null)}>
              <option value="">—</option>
              {USE_TYPES.map((u) => (
                <option key={u} value={u}>
                  {u}
                </option>
              ))}
            </Select>
          </Field>
          <Field label="Operational Status">
            <Select
              value={form.operational_status ?? ""}
              onChange={(e) => set("operational_status", e.target.value || null)}
            >
              <option value="">—</option>
              {OP_STATUSES.map((s) => (
                <option key={s} value={s}>
                  {s}
                </option>
              ))}
            </Select>
            <RbOverride
              show={!!channel?.operational_status_overridden && channel?.rb_operational_status != null}
              rbValue={channel?.rb_operational_status ?? ""}
              onAccept={() => acceptRb("operational_status")}
            />
          </Field>
          <Field label="Service">
            <Select value={form.service_type ?? ""} onChange={(e) => set("service_type", e.target.value || null)}>
              {SERVICE_TYPES.map((s) => (
                <option key={s} value={s}>
                  {s}
                </option>
              ))}
            </Select>
          </Field>
        </Section>

        <Section title="Mode">
          <Field label="Mode">
            <Select value={form.mode ?? "FM"} onChange={(e) => set("mode", e.target.value)}>
              {MODES.map((m) => (
                <option key={m} value={m}>
                  {m}
                </option>
              ))}
            </Select>
          </Field>
          {form.mode === "DMR" && (
            <Field label="Color Code">
              <TextInput
                value={form.dmr_color_code != null ? String(form.dmr_color_code) : ""}
                placeholder="0–15"
                onChange={(e) => set("dmr_color_code", num(e.target.value))}
              />
            </Field>
          )}
          {form.mode === "P25" && (
            <Field label="NAC">
              <TextInput
                value={form.p25_nac ?? ""}
                placeholder="293"
                onChange={(e) => set("p25_nac", e.target.value || null)}
              />
            </Field>
          )}
          {form.mode === "M17" && (
            <Field label="CAN">
              <TextInput
                value={form.m17_can != null ? String(form.m17_can) : ""}
                placeholder="0–15"
                onChange={(e) => set("m17_can", num(e.target.value))}
              />
            </Field>
          )}
        </Section>

        {form.mode === "DMR" && (
          <div className="space-y-2">
            <h3 className="text-[11px] font-semibold uppercase tracking-wide text-slate-400">
              Talkgroups on this repeater
            </h3>
            <p className="text-[11px] leading-snug text-slate-400">
              List the talkgroups this repeater carries. On export, each one
              becomes its own radio channel (one talkgroup + timeslot per
              channel) — the radio can't hold multiple talkgroups on a single
              channel.
            </p>
            {mode === "edit" && channel ? (
              <TalkgroupAssignments channelId={channel.id} />
            ) : (
              <p className="text-xs text-slate-400">
                Save the repeater first, then reopen it to assign DMR talkgroups.
              </p>
            )}
          </div>
        )}

        <div className="space-y-2">
          <h3 className="text-[11px] font-semibold uppercase tracking-wide text-slate-400">
            Notes
          </h3>
          <textarea
            className="w-full rounded-md border border-slate-300 bg-white px-2.5 py-1.5 text-xs text-slate-800 focus:border-sky-500 focus:outline-none focus:ring-1 focus:ring-sky-500 dark:border-slate-600 dark:bg-slate-900 dark:text-slate-100"
            rows={3}
            value={form.notes ?? ""}
            onChange={(e) => set("notes", e.target.value || null)}
          />
          <RbOverride
            show={!!channel?.notes_overridden && channel?.rb_notes != null}
            rbValue={channel?.rb_notes ?? ""}
            onAccept={() => acceptRb("notes")}
          />
        </div>
      </div>
    </SlideOver>
  );
}

/// Manage the DMR talkgroups assigned to a repeater (channel). Lets the user
/// pick talkgroups from the library, choose a timeslot, and remove them.
function TalkgroupAssignments({ channelId }: { channelId: number }) {
  const [assignments, setAssignments] = useState<RepeaterTalkgroup[]>([]);
  const [library, setLibrary] = useState<Talkgroup[]>([]);
  const [loading, setLoading] = useState(true);
  const [pickTg, setPickTg] = useState("");
  const [pickTs, setPickTs] = useState(1);
  const [busy, setBusy] = useState(false);

  const reload = async () => {
    setAssignments(await api.getRepeaterTalkgroups(channelId));
  };

  useEffect(() => {
    let active = true;
    setLoading(true);
    Promise.all([api.getRepeaterTalkgroups(channelId), api.listTalkgroups()])
      .then(([a, lib]) => {
        if (!active) return;
        setAssignments(a);
        setLibrary(lib);
      })
      .finally(() => active && setLoading(false));
    return () => {
      active = false;
    };
  }, [channelId]);

  const add = async () => {
    const tgId = parseInt(pickTg, 10);
    if (!Number.isFinite(tgId)) return;
    setBusy(true);
    const res = await withToast(api.assignTalkgroup(channelId, tgId, pickTs), {
      error: "Could not assign talkgroup",
    });
    setBusy(false);
    if (res !== undefined) {
      setPickTg("");
      await reload();
    }
  };

  const remove = async (assignmentId: number) => {
    const res = await withToast(api.removeTalkgroupAssignment(assignmentId));
    if (res !== undefined)
      setAssignments((prev) => prev.filter((a) => a.id !== assignmentId));
  };

  if (loading) return <p className="text-xs text-slate-400">Loading…</p>;

  return (
    <div className="space-y-2">
      {assignments.length === 0 ? (
        <p className="text-xs text-slate-400">No talkgroups assigned yet.</p>
      ) : (
        <>
        <p className="text-[11px] font-medium text-sky-600 dark:text-sky-400">
          Exports as {assignments.length} channel
          {assignments.length === 1 ? "" : "s"} on the radio.
        </p>
        <div className="overflow-hidden rounded-md border border-slate-200 dark:border-slate-700">
          {assignments.map((a) => (
            <div
              key={a.id}
              className="flex items-center justify-between gap-2 border-b border-slate-100 px-3 py-1.5 text-xs last:border-b-0 dark:border-slate-700/60"
            >
              <span className="flex items-center gap-2 truncate">
                <span className="font-mono tabular-nums text-slate-500">
                  {a.tg_number}
                </span>
                <span className="font-medium text-slate-800 dark:text-slate-100">
                  {a.name}
                </span>
                <Badge>TS{a.timeslot}</Badge>
                <span className="text-[11px] text-slate-400">{a.network}</span>
              </span>
              <button
                onClick={() => remove(a.id)}
                className="shrink-0 rounded p-0.5 text-slate-400 hover:bg-slate-100 hover:text-rose-600 dark:hover:bg-slate-700"
                title="Remove"
              >
                <X size={13} />
              </button>
            </div>
          ))}
        </div>
        </>
      )}

      {library.length === 0 ? (
        <p className="text-[11px] text-slate-400">
          No talkgroups in the library yet — add some on the Talkgroups page.
        </p>
      ) : (
        <div className="flex items-end gap-2">
          <label className="flex flex-1 flex-col gap-1">
            <span className="text-[11px] font-medium text-slate-500 dark:text-slate-400">
              Talkgroup
            </span>
            <TalkgroupCombobox
              talkgroups={library}
              value={pickTg}
              onChange={setPickTg}
            />
          </label>
          <label className="flex w-20 flex-col gap-1">
            <span className="text-[11px] font-medium text-slate-500 dark:text-slate-400">
              Slot
            </span>
            <Select
              value={String(pickTs)}
              onChange={(e) => setPickTs(Number(e.target.value))}
            >
              {TIMESLOTS.map((ts) => (
                <option key={ts} value={ts}>
                  TS{ts}
                </option>
              ))}
            </Select>
          </label>
          <Button
            variant="primary"
            onClick={add}
            disabled={busy || pickTg === ""}
          >
            <Plus size={14} /> Add
          </Button>
        </div>
      )}
    </div>
  );
}

// Cap on rendered matches — the library can hold thousands of talkgroups, so we
// never paint more than this many rows at once.
const TG_MATCH_LIMIT = 200;

const tgLabel = (t: Talkgroup) => `${t.tg_number} · ${t.name} (${t.network})`;

/// A type-to-filter talkgroup picker. The native <select> is unusable with
/// 1000+ rows (it only prefix-matches single keystrokes); this filters the
/// whole library by number, name, or network as you type.
function TalkgroupCombobox({
  talkgroups,
  value,
  onChange,
}: {
  talkgroups: Talkgroup[];
  value: string;
  onChange: (id: string) => void;
}) {
  const [query, setQuery] = useState("");
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(0);
  const rootRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  const selected = useMemo(
    () => talkgroups.find((t) => String(t.id) === value) ?? null,
    [talkgroups, value],
  );

  // Reflect external selection/clearing in the input while it's closed.
  useEffect(() => {
    if (!open) setQuery(selected ? tgLabel(selected) : "");
  }, [selected, open]);

  const results = useMemo(() => {
    const q = query.trim().toLowerCase();
    // No query, or the box still shows the current selection's label → show all.
    if (!q || (selected && q === tgLabel(selected).toLowerCase())) {
      return talkgroups.slice(0, TG_MATCH_LIMIT);
    }
    const out: Talkgroup[] = [];
    for (const t of talkgroups) {
      const hay = `${t.tg_number} ${t.name} ${t.network}`.toLowerCase();
      if (hay.includes(q)) {
        out.push(t);
        if (out.length >= TG_MATCH_LIMIT) break;
      }
    }
    return out;
  }, [talkgroups, query, selected]);

  // Close on outside click, restoring the input to the current selection.
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) {
        setOpen(false);
        setQuery(selected ? tgLabel(selected) : "");
      }
    };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, [open, selected]);

  // Keep the active row in bounds whenever the result set changes.
  useEffect(() => {
    setActive((a) => Math.min(a, Math.max(0, results.length - 1)));
  }, [results.length]);

  const choose = (t: Talkgroup) => {
    onChange(String(t.id));
    setQuery(tgLabel(t));
    setOpen(false);
    inputRef.current?.blur();
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setOpen(true);
      setActive((a) => Math.min(a + 1, results.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActive((a) => Math.max(a - 1, 0));
    } else if (e.key === "Enter") {
      if (open && results[active]) {
        e.preventDefault();
        choose(results[active]);
      }
    } else if (e.key === "Escape") {
      setOpen(false);
      setQuery(selected ? tgLabel(selected) : "");
    }
  };

  return (
    <div ref={rootRef} className="relative">
      <TextInput
        ref={inputRef}
        className="w-full"
        placeholder="Type a number or name…"
        value={query}
        onChange={(e) => {
          setQuery(e.target.value);
          setOpen(true);
          setActive(0);
          if (value) onChange(""); // typing invalidates the prior pick
        }}
        onFocus={(e) => {
          setOpen(true);
          e.target.select();
        }}
        onKeyDown={onKeyDown}
      />
      {open && (
        <div className="absolute z-20 mt-1 max-h-64 w-full overflow-auto rounded-md border border-slate-200 bg-white py-1 shadow-lg dark:border-slate-700 dark:bg-slate-800">
          {results.length === 0 ? (
            <div className="px-3 py-2 text-xs text-slate-400">No matches</div>
          ) : (
            results.map((t, i) => (
              <button
                key={t.id}
                type="button"
                // Use mousedown so selection fires before the input blurs.
                onMouseDown={(e) => {
                  e.preventDefault();
                  choose(t);
                }}
                onMouseEnter={() => setActive(i)}
                className={clsx(
                  "flex w-full items-center gap-2 px-3 py-1.5 text-left text-xs",
                  i === active
                    ? "bg-sky-50 dark:bg-sky-950/50"
                    : "hover:bg-slate-50 dark:hover:bg-slate-700/40",
                )}
              >
                <span className="w-14 shrink-0 font-mono tabular-nums text-slate-500">
                  {t.tg_number}
                </span>
                <span className="flex-1 truncate font-medium text-slate-800 dark:text-slate-100">
                  {t.name}
                </span>
                <span className="shrink-0 text-[11px] text-slate-400">
                  {t.network}
                </span>
              </button>
            ))
          )}
          {results.length >= TG_MATCH_LIMIT && (
            <div className="border-t border-slate-100 px-3 py-1.5 text-[11px] text-slate-400 dark:border-slate-700">
              Showing first {TG_MATCH_LIMIT} — keep typing to narrow.
            </div>
          )}
        </div>
      )}
    </div>
  );
}
