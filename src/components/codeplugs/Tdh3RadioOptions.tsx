import { useEffect, useState } from "react";
import {
  Upload,
  CheckCircle2,
  XCircle,
  AlertTriangle,
  UserCog,
} from "lucide-react";
import { api } from "../../lib/api";
import type { RadioProfile, Tdh3ProfileApplyResult } from "../../lib/types";
import { Button, Spinner, Select } from "../ui";

/**
 * TD-H3 Phase C: apply a saved radio profile's settings straight to the radio
 * (channels untouched). Reading the radio's settings and saving them into a
 * profile is done in the profile editor under Radios (the same flow as the
 * UV-5R), so this tab is just the "push a profile to the radio" direction.
 * Every apply downloads + backs up the full image first, patches only the
 * settings bits, uploads, then reads back to verify.
 */
export function Tdh3RadioOptions({
  port,
  modelName,
  modelId,
}: {
  port: string;
  modelName: string;
  modelId: number;
}) {
  const [busy, setBusy] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const [result, setResult] = useState<
    | null
    | { verified: boolean; message: string }
  >(null);
  const [error, setError] = useState<string | null>(null);

  // Saved profiles for this radio model (for "Apply profile").
  const [profiles, setProfiles] = useState<RadioProfile[]>([]);
  const [profileId, setProfileId] = useState<number | "">("");

  // Load this radio model's saved profiles once (filtered by model id, not name).
  useEffect(() => {
    (async () => {
      try {
        const allProfiles = await api.listRadioProfiles();
        const mine = allProfiles.filter((p) => p.radio_model_id === modelId);
        setProfiles(mine);
        setProfileId((cur) => (cur === "" ? (mine[0]?.id ?? "") : cur));
      } catch {
        /* leave profiles empty; the tab still renders */
      }
    })();
  }, [modelId]);

  // Reset transient state whenever the dialog reopens against a (maybe) new port.
  useEffect(() => {
    setResult(null);
    setConfirming(false);
    setError(null);
  }, [port]);

  const doApply = async () => {
    if (profileId === "") return;
    setError(null);
    setConfirming(false);
    setBusy(true);
    try {
      const res: Tdh3ProfileApplyResult = await api.applyTdh3Profile(port, profileId);
      setResult({
        verified: res.verified,
        message: res.verified
          ? `Applied ${res.applied} setting${res.applied === 1 ? "" : "s"} from the profile · verified ✓`
          : res.verify_note ||
            `Applied ${res.applied} setting${res.applied === 1 ? "" : "s"} — verification warning`,
      });
    } catch (e) {
      setError(typeof e === "string" ? e : String(e));
    } finally {
      setBusy(false);
    }
  };

  const selectedProfile = profiles.find((p) => p.id === profileId);

  return (
    <div className="px-5 pb-5">
      {/* Apply a saved profile (settings-only, channels untouched) */}
      <div className="mb-3 rounded-md border border-slate-200 bg-slate-50 p-3 dark:border-slate-700 dark:bg-slate-800/40">
        <div className="mb-2 flex items-center gap-1.5 text-[11px] font-semibold uppercase tracking-wide text-slate-500 dark:text-slate-400">
          <UserCog size={12} /> Apply a saved radio profile
        </div>
        <p className="mb-2 text-xs text-slate-500 dark:text-slate-400">
          Write the settings from a profile you configured under <strong>Radios</strong>{" "}
          straight to the radio. Channels are left untouched. To capture the radio's
          current settings into a profile, open the profile under <strong>Radios</strong>{" "}
          and use <strong>Download from radio</strong>.
        </p>
        <div className="flex flex-wrap items-end gap-2">
          <label className="flex-1">
            <span className="mb-1 block text-[11px] font-medium text-slate-500 dark:text-slate-400">
              Profile
            </span>
            <Select
              className="w-full"
              value={profileId === "" ? "" : String(profileId)}
              onChange={(e) =>
                setProfileId(e.target.value === "" ? "" : Number(e.target.value))
              }
            >
              {profiles.length === 0 && (
                <option value="">No saved {modelName} profiles</option>
              )}
              {profiles.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.display_name}
                </option>
              ))}
            </Select>
          </label>
          <Button
            variant="primary"
            onClick={() => setConfirming(true)}
            disabled={!port || busy || profileId === ""}
            title={profileId === "" ? "Pick a saved profile first" : undefined}
          >
            {busy ? <Spinner className="h-3.5 w-3.5" /> : <Upload size={14} />}
            Apply profile to radio
          </Button>
        </div>
      </div>

      {error && (
        <div className="mb-4 flex items-start gap-2 rounded-md border border-red-200 bg-red-50 px-3 py-2 text-xs text-red-600 dark:border-red-900/50 dark:bg-red-950/40 dark:text-red-300">
          <XCircle size={14} className="mt-px shrink-0" />
          <span>{error}</span>
        </div>
      )}

      {confirming && (
        <div className="mb-4 rounded-md border border-amber-300 bg-amber-50 p-3 text-xs dark:border-amber-900/50 dark:bg-amber-950/40">
          <div className="mb-2 flex items-center gap-1.5 font-semibold text-amber-800 dark:text-amber-300">
            <AlertTriangle size={14} />{" "}
            {`Apply “${selectedProfile?.display_name ?? "profile"}” to ${modelName}`}
          </div>
          <p className="text-amber-800 dark:text-amber-200">
            This writes the profile's settings to the radio. Your channels are left
            untouched, and a full backup is saved first.
          </p>
          <div className="mt-3 flex justify-end gap-2">
            <Button variant="ghost" onClick={() => setConfirming(false)}>
              Cancel
            </Button>
            <Button variant="primary" onClick={doApply}>
              <Upload size={14} /> Write to radio
            </Button>
          </div>
        </div>
      )}

      {busy && (
        <div className="mb-4 flex items-center gap-2 rounded-md border border-slate-200 bg-slate-50 px-3 py-2 text-xs text-slate-600 dark:border-slate-700 dark:bg-slate-800/50 dark:text-slate-300">
          <Spinner className="h-3.5 w-3.5" />
          Backing up → writing settings → verifying… keep the radio on and the
          cable connected.
        </div>
      )}

      {result && (
        <div
          className={
            result.verified
              ? "mb-4 flex items-center gap-2 rounded-md border border-emerald-200 bg-emerald-50 px-3 py-2 text-xs text-emerald-700 dark:border-emerald-900/50 dark:bg-emerald-950/40 dark:text-emerald-300"
              : "mb-4 flex items-start gap-2 rounded-md border border-amber-300 bg-amber-50 px-3 py-2 text-xs text-amber-800 dark:border-amber-900/50 dark:bg-amber-950/40 dark:text-amber-200"
          }
        >
          {result.verified ? (
            <CheckCircle2 size={14} className="shrink-0" />
          ) : (
            <AlertTriangle size={14} className="mt-px shrink-0" />
          )}
          <span>{result.message}</span>
        </div>
      )}
    </div>
  );
}
