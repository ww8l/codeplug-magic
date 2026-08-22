import { useState } from "react";
import {
  Sun,
  Moon,
  Monitor,
  Settings as SettingsIcon,
  Download,
  Upload,
} from "lucide-react";
import clsx from "clsx";
import { save, open, confirm as confirmDialog } from "@tauri-apps/plugin-dialog";
import { toast } from "sonner";
import { useTheme } from "../theme";
import { api } from "../lib/api";
import type { BackupTable } from "../lib/types";
import { useOnlineGeocoding } from "../lib/prefs";
import { Button, Card, PageHeader, APP_VERSION, Spinner } from "../components/ui";

const THEMES = [
  { value: "light", label: "Light", icon: Sun },
  { value: "dark", label: "Dark", icon: Moon },
  { value: "system", label: "System", icon: Monitor },
] as const;

function todayStamp() {
  return new Date().toISOString().slice(0, 10);
}

export function Settings() {
  const { theme, setTheme } = useTheme();
  const [busy, setBusy] = useState<null | "backup" | "restore">(null);
  const [geocodeOnline, setGeocodeOnline] = useOnlineGeocoding();

  // Name what is about to be in the file BEFORE it exists, not after it has
  // been mailed to someone (#81). The inventory comes from the live schema, so
  // this stays honest as the database grows; the cautions are the parts nobody
  // would predict from "back up my codeplugs" — chiefly the radioid.net dump,
  // which is other operators' names and cities riding along.
  const describeBackup = (contents: BackupTable[]) => {
    const rows = contents
      .filter((t) => t.rows > 0)
      .map((t) => `    ${t.rows.toLocaleString()} ${t.label.toLowerCase()}`);
    const cautions = contents
      .filter((t) => t.rows > 0 && t.caution)
      .map((t) => `    • ${t.label}: ${t.caution}`);
    return [
      "A backup is a copy of the whole database. This one will contain:",
      "",
      ...rows,
      ...(cautions.length
        ? ["", "Worth knowing before you share the file:", "", ...cautions]
        : []),
      "",
      "Write the backup?",
    ].join("\n");
  };

  const doBackup = async () => {
    let contents: BackupTable[];
    try {
      contents = await api.backupContents();
    } catch (e) {
      toast.error(`Could not read what the backup would contain: ${e}`);
      return;
    }
    const ok = await confirmDialog(describeBackup(contents), {
      title: "Back up database",
      kind: "info",
    });
    if (!ok) return;

    const path = await save({
      defaultPath: `codeplug-backup-${todayStamp()}.sqlite3`,
      filters: [{ name: "Codeplug Magic backup", extensions: ["sqlite3"] }],
    });
    if (!path) return;
    setBusy("backup");
    try {
      await api.exportDatabase(path);
      toast.success("Full database backed up");
    } catch (e) {
      toast.error(`Backup failed: ${e}`);
    } finally {
      setBusy(null);
    }
  };

  const doRestore = async () => {
    const path = await open({
      multiple: false,
      filters: [{ name: "Codeplug Magic backup", extensions: ["sqlite3"] }],
    });
    if (!path || typeof path !== "string") return;

    if (!(await api.isDatabaseBackup(path))) {
      toast.error("That file is not a WW8L Codeplug Magic database backup.");
      return;
    }

    const ok = await confirmDialog(
      "Restoring replaces your ENTIRE current database with this backup. " +
        "Everything now in the app will be overwritten. Continue?",
      { title: "Restore database", kind: "warning" },
    );
    if (!ok) return;

    setBusy("restore");
    try {
      const snapshot = await api.importDatabase(path);
      toast.success("Database restored — reloading…", {
        description: `Your previous data was saved to ${snapshot}`,
      });
      // Reload the webview so every view re-queries the restored data.
      setTimeout(() => window.location.reload(), 400);
    } catch (e) {
      setBusy(null);
      toast.error(`Restore failed: ${e}`);
    }
  };

  return (
    <>
      <PageHeader icon={<SettingsIcon size={18} />} title="Settings" />
      <div className="flex-1 overflow-auto p-5">
        <div className="max-w-2xl space-y-5">
          <Card className="p-4">
            <h2 className="text-sm font-semibold text-slate-800 dark:text-slate-100">
              Appearance
            </h2>
            <p className="mt-0.5 text-xs text-slate-500 dark:text-slate-400">
              Defaults to your system preference.
            </p>
            <div className="mt-3 inline-flex rounded-md border border-slate-200 p-0.5 dark:border-slate-700">
              {THEMES.map(({ value, label, icon: Icon }) => (
                <button
                  key={value}
                  onClick={() => setTheme(value)}
                  className={clsx(
                    "flex items-center gap-1.5 rounded px-3 py-1.5 text-xs font-medium transition-colors",
                    theme === value
                      ? "bg-sky-600 text-white"
                      : "text-slate-600 hover:bg-slate-100 dark:text-slate-300 dark:hover:bg-slate-800",
                  )}
                >
                  <Icon size={14} /> {label}
                </button>
              ))}
            </div>
          </Card>

          <Card className="p-4">
            <h2 className="text-sm font-semibold text-slate-800 dark:text-slate-100">
              Database
            </h2>
            <p className="mt-1 text-xs text-slate-500 dark:text-slate-400">
              Your master database is stored in the application data directory as{" "}
              <code className="rounded bg-slate-100 px-1 py-0.5 font-mono text-[11px] dark:bg-slate-700">
                codeplug_manager.sqlite3
              </code>
              . A backup is a single file containing <strong>100% of your
              data</strong> — every channel, list, codeplug, radio profile,
              talkgroup and DMR contact.
            </p>
            <p className="mt-1.5 text-xs text-slate-500 dark:text-slate-400">
              That includes your call sign, DMR ID and any APRS position, and
              the whole imported radioid.net contact database — other operators’
              names and cities. Backing up lists it all first, so you know what
              you are handing over if you send the file to someone.
            </p>
            <div className="mt-3 flex flex-wrap gap-2">
              <Button
                variant="primary"
                onClick={doBackup}
                disabled={busy !== null}
              >
                {busy === "backup" ? (
                  <Spinner className="h-3.5 w-3.5" />
                ) : (
                  <Download size={14} />
                )}
                Back up database…
              </Button>
              <Button onClick={doRestore} disabled={busy !== null}>
                {busy === "restore" ? (
                  <Spinner className="h-3.5 w-3.5" />
                ) : (
                  <Upload size={14} />
                )}
                Restore from backup…
              </Button>
            </div>
            <p className="mt-2 text-[11px] text-slate-400 dark:text-slate-500">
              Restoring replaces everything currently in the app, then reloads.
            </p>
          </Card>

          <Card className="p-4">
            <h2 className="text-sm font-semibold text-slate-800 dark:text-slate-100">
              Privacy
            </h2>
            <p className="mt-1 text-xs text-slate-500 dark:text-slate-400">
              This app has no telemetry, no accounts and no automatic updates.
              It makes exactly three kinds of network request, every one of
              them because you asked for something:
            </p>
            <ul className="mt-2 space-y-1 text-xs text-slate-500 dark:text-slate-400">
              <li>
                <strong className="font-semibold text-slate-700 dark:text-slate-200">
                  Coordinate lookup
                </strong>{" "}
                — the town name you typed goes to OpenStreetMap’s Nominatim
                service, which answers with a latitude and longitude. Only the
                town, state and country are sent; never a call sign, a
                frequency, or anything identifying you.
              </li>
              <li>
                <strong className="font-semibold text-slate-700 dark:text-slate-200">
                  DMR contact refresh
                </strong>{" "}
                — downloads the public radioid.net user database. Nothing about
                you is sent.
              </li>
              <li>
                <strong className="font-semibold text-slate-700 dark:text-slate-200">
                  Talkgroup download
                </strong>{" "}
                — fetches the public BrandMeister talkgroup list when you press
                BrandMeister on the Talkgroups page. The list is not shipped
                with the app. Nothing about you is sent.
              </li>
            </ul>
            <label className="mt-3 flex items-start gap-2 text-xs text-slate-600 dark:text-slate-300">
              <input
                type="checkbox"
                className="mt-0.5 h-3.5 w-3.5 accent-sky-600"
                checked={geocodeOnline}
                onChange={(e) => setGeocodeOnline(e.target.checked)}
              />
              <span>
                Look up coordinates online when a town isn’t already in my
                channels.
                <span className="mt-0.5 block text-[11px] text-slate-400 dark:text-slate-500">
                  Turn this off and no town name ever leaves this machine.
                  Coordinates are still filled in from towns you already have
                  repeaters in — that match is local and instant — and you can
                  always type a latitude and longitude yourself.
                </span>
              </span>
            </label>
          </Card>

          <Card className="p-4">
            <h2 className="text-sm font-semibold text-slate-800 dark:text-slate-100">
              About
            </h2>
            <p className="mt-1 text-xs text-slate-500 dark:text-slate-400">
              WW8L Codeplug Magic v{APP_VERSION} — a single master database and
              intelligent per-radio export for amateur radio operators.
            </p>
          </Card>
        </div>
      </div>
    </>
  );
}
