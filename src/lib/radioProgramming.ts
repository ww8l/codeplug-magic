/**
 * Frontend half of the driver registry (Chunk 3.7).
 *
 * The backend picks a driver from `radio_models.driver_key`; the UI picks a
 * "Program radio" dialog from `radio_models.programming_ui` — the sibling
 * column added for exactly this in migration 0014. Adding a radio therefore
 * means a driver folder under `src-tauri/src/radios/<key>/`, one line in
 * `all_drivers()`, and — only if it needs a bespoke dialog — one line in
 * `PROGRAM_DIALOGS` below. Never a new arm in `Codeplugs.tsx`.
 *
 * A model with no `programming_ui` (or an unrecognised one) gets the generic
 * capability-driven dialog, which works for any registered driver with no new
 * frontend code at all.
 */
import { useEffect, useState, type ReactElement } from "react";
import { api } from "./api";
import type { DriverCapabilities, RadioModel } from "./types";

/// Every program dialog takes exactly this. A dialog reads what it needs off
/// `model` (`driver_key` to dispatch, `id` and `display_name` to label itself)
/// so the registry never has to know which props a given dialog wants.
export interface ProgramDialogProps {
  open: boolean;
  onClose: () => void;
  codeplugId: number;
  codeplugName: string;
  /// The codeplug's radio model. Null while it is still loading, or if the row
  /// has gone missing — dialogs must render something harmless in that case.
  model: RadioModel | null;
  /// The codeplug's radio profile. Settings and the call-sign DB are written
  /// per PROFILE (3.6d); the channel-set program stays codeplug-scoped.
  profileId: number | null;
}

/// A registered dialog. The `programming_ui` → component map itself lives in
/// `components/codeplugs/programDialogs.ts`, so this module holds only types
/// and data and never pulls component code into whatever imports it.
export type ProgramDialog = (props: ProgramDialogProps) => ReactElement;

/// The driver key a dialog should dispatch to, or "" when the model is still
/// loading or is export-only. Commands reject "" by name rather than opening a
/// port, so this is safe to pass straight through.
export function driverKeyOf(model: RadioModel | null): string {
  return model?.driver_key ?? "";
}

/// Can this model be programmed over a cable at all? False for export-only
/// models (NULL `driver_key`), which is what the generic dialog gates on.
export function isProgrammable(model: RadioModel | null): boolean {
  return model?.driver_key != null;
}

/// Fetch a driver's capability flags. They are derived from the Rust trait
/// impls and never change at runtime, so results are cached per key for the
/// life of the process and each key is fetched at most once.
const capsCache = new Map<string, DriverCapabilities>();

export function useDriverCapabilities(
  driverKey: string | null,
): DriverCapabilities | null {
  const [caps, setCaps] = useState<DriverCapabilities | null>(() =>
    driverKey ? (capsCache.get(driverKey) ?? null) : null,
  );

  useEffect(() => {
    if (!driverKey) {
      setCaps(null);
      return;
    }
    const cached = capsCache.get(driverKey);
    if (cached) {
      setCaps(cached);
      return;
    }
    let live = true;
    api
      .driverCapabilities(driverKey)
      .then((c) => {
        capsCache.set(driverKey, c);
        if (live) setCaps(c);
      })
      // A missing driver is not an error the user can act on here — the
      // dialog's own actions report it when they are actually invoked.
      .catch(() => {
        if (live) setCaps(null);
      });
    return () => {
      live = false;
    };
  }, [driverKey]);

  return caps;
}
