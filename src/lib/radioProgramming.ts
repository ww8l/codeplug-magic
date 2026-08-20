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

/// A radio programmed by writing a file onto its own removable media instead of
/// over a cable. This is still *programming* from the user's point of view — it
/// ends with a codeplug on the radio — so it belongs in the Program dialog next
/// to the cable actions, not hidden behind Export.
export interface MediaWrite {
  /// Action button label, e.g. "Write to SD card…".
  action: string;
  /// Title of the OS file picker.
  pickTitle: string;
  /// Extensions the picker accepts, and what to call them.
  filterName: string;
  extensions: string[];
  /// What the user does *on the radio* before and after. Both are front-panel
  /// menu steps, not app actions — the app never talks to a media radio, it
  /// only edits a file the radio itself wrote and will read back.
  before: string;
  after: string;
  /// Shown once the card has been found for them, in place of `before`.
  found: string;
  /// What survives the patch, and what the untouched copy is called. Every
  /// media radio is patched in place, and every one of them keeps settings this
  /// app has never heard of, so saying which is per-radio and not optional.
  preserves: string;
  /// The same fact read the other way round, for when the file is shared (#75).
  ///
  /// `preserves` is the reassurance — nothing you set on the radio is lost.
  /// This is the consequence: because the output is a patched COPY of the
  /// operator's own file, everything personal in it is copied too. A club
  /// codeplug posted to a mailing list carries the exporter's identity with it,
  /// and nothing in the flow used to say so.
  carries: string;
}

/// The `export_format` → media-write map. Keyed on the format rather than the
/// model so this is the one place a media-programmed radio is described, the
/// same bargain `PROGRAM_DIALOGS` makes for bespoke dialogs — the dialogs
/// themselves stay radio-agnostic.
const MEDIA_WRITES: Record<string, MediaWrite> = {
  yaesu_ft5d_sd: {
    action: "Write to SD card…",
    pickTitle: "Select FT5D/BACKUP/BACKUP.dat on the radio’s microSD card",
    filterName: "FT5D backup",
    extensions: ["dat"],
    // Menu paths are the ones in scratchpad/ft5d/CAPTURE-CHECKLIST.md, taken
    // off the radio itself. Firmware wording varies, so they read as a
    // direction to follow on screen rather than an exact incantation.
    before:
      "On the radio, save its memory to the card: SD CARD menu → Backup → Memory → SD card. Then eject the card, put it in this computer, and pick FT5D/BACKUP/BACKUP.dat below.",
    after:
      "Put the card back in the radio and load it: SD CARD menu → Backup → SD card → Memory.",
    found:
      "It already holds a backup, so there is nothing to pick — just check it is current (the radio writes it with SD CARD menu → Backup → Memory → SD card).",
    preserves:
      "Channel lists become banks, and everything else already on the radio — APRS, GPS, WIRES-X — is left untouched. The first write saves your untouched file beside it as BACKUP.dat.orig; later writes never overwrite that pristine copy.",
    carries:
      "your MYCALL, your APRS call sign and SSID, and the APRS and GPS settings the radio had stored",
  },
  // The ID-52 is the second card radio, and the first whose ONE file carries
  // both halves of a codeplug: Load Setting → ALL restores the memories and
  // every MENU setting together, so the profile's settings ride along with the
  // channels instead of needing a second trip.
  icom_id52_icf: {
    action: "Write to microSD…",
    pickTitle:
      "Select a settings file from ID-52/Setting/ on the radio’s microSD card — it will be rewritten in place",
    filterName: "ID-52 settings file",
    // The Memory CH CSV is the radio's other card file — memories only, and
    // imported separately. The exporter writes whichever one is picked.
    extensions: ["icf", "csv"],
    // Menu paths are the Advanced Manual's, cross-checked against Tim's radio.
    before:
      "On the radio, save its settings to the card: MENU → SET → SD Card → Save Setting. Then put the card in this computer — or connect the radio with SET → Function → USB Connect → SD Card Mode — and pick that file from ID-52/Setting/ below.",
    after:
      "Put the card back in the radio and load it: MENU → SET → SD Card → Load Setting → pick the file → ALL. That restores the memories and the settings together.",
    found:
      "It already holds a settings file, so there is nothing to pick — just check it is current (the radio writes one with MENU → SET → SD Card → Save Setting).",
    preserves:
      "Channel lists become memory groups, and everything else in the file — the repeater list, your call signs, GPS and Bluetooth — is carried across untouched. Writing to the card creates a NEW settings file named the way the radio names its own, built from the newest one there, so nothing you saved is modified at all.",
    carries:
      "your D-STAR and APRS call signs, your GPS and Bluetooth settings, and the rest of the radio's own configuration",
  },
  // The third card radio. Like the ID-52 it keeps everything in one file the
  // radio writes and reads back, and like the ID-52 the card can stay in the
  // radio — USB-C mounts it as a drive.
  kenwood_thd75_sd: {
    action: "Write to microSD…",
    // Picking a file identifies the card, it does not nominate a file to
    // overwrite — the write always creates a new save beside it, so the title
    // says so rather than implying the picked file is about to be rewritten.
    pickTitle:
      "Select any config file from KENWOOD/TH-D75/SETTINGS/DATA/ on the radio’s microSD card — a new one will be written beside it",
    filterName: "TH-D75 config file",
    extensions: ["d75"],
    // Menu paths are the TH-D75 manual's, confirmed against Tim's radio.
    before:
      "On the radio, save its configuration to the card: Menu → Configuration → SD Card → Save Setting. Then put the card in this computer — or connect the radio over USB-C and switch it to SD card mode — and pick that file from KENWOOD/TH-D75/SETTINGS/DATA/ below.",
    after:
      "Put the card back in the radio and load it: Menu → Configuration → SD Card → Load Setting → pick the newest file. Then check the memory list in each group, not just the one the radio comes up on.",
    found:
      "It already holds a config file, so there is nothing to pick — just check it is current (the radio writes one with Menu → Configuration → SD Card → Save Setting).",
    preserves:
      "Channel lists become memory groups, and everything else in the file — APRS, your call sign history, the repeater list and every menu setting — is carried across untouched. Writing to the card creates a NEW config file named the way the radio names its own, built from the newest one there, so nothing you saved is modified at all.",
    carries:
      "your APRS and D-STAR call signs, your call sign history, your Bluetooth device name, and any APRS positions stored in the radio — beacon objects are held to 1/10000 of a minute, which for a home station is a street address",
  },
};

/// How a model is programmed from removable media, or null if it is a cable
/// radio. Callers must handle null — most radios have no media path.
export function mediaWriteFor(model: RadioModel | null): MediaWrite | null {
  return mediaWriteForFormat(model?.export_format ?? null);
}

/// The same lookup for callers holding only an export format — the export
/// preview carries one but no model row.
export function mediaWriteForFormat(format: string | null): MediaWrite | null {
  return (format && MEDIA_WRITES[format]) || null;
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
