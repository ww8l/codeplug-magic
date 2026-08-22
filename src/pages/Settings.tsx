import { useCallback, useEffect, useState } from "react";
import {
  Sun,
  Moon,
  Monitor,
  Settings as SettingsIcon,
  Download,
  Upload,
  Eraser,
} from "lucide-react";
import clsx from "clsx";
import { save, open, confirm as confirmDialog } from "@tauri-apps/plugin-dialog";
import { toast } from "sonner";
import { useTheme } from "../theme";
import { api } from "../lib/api";
import { Button, Card, PageHeader, APP_VERSION, Spinner, Select } from "../components/ui";
import type { RadioBackupsSummary } from "../lib/types";

const THEMES = [
  { value: "light", label: "Light", icon: Sun },
  { value: "dark", label: "Dark", icon: Moon },
  { value: "system", label: "System", icon: Monitor },
] as const;

function todayStamp() {
  return new Date().toISOString().slice(0, 10);
}

/** Bytes as the operator would say them: 37.2 MB, not 39,006,208. */
function humanBytes(bytes: number): string {
  const units = ["bytes", "KB", "MB", "GB"];
  let n = bytes;
  let u = 0;
  while (n >= 1024 && u < units.length - 1) {
    n /= 1024;
    u += 1;
  }
  return `${u === 0 ? n : n.toFixed(1)} ${units[u]}`;
}

const KEEP_CHOICES = [1, 3, 5, 10, 20];

/**
 * What has accumulated in `radio-backups/`, and the only thing in the app that
 * deletes any of it.
 *
 * Every read from a radio and every write to one drops a full copy of that
 * radio's memory here, and nothing ever removed one — 144 files and 37 MB on
 * the machine this was written on (#77). They are worth keeping: a backup is
 * the way back after a bad write. They were not worth keeping INVISIBLY, in a
 * folder the operator has no reason to look in, holding their call sign, DMR
 * ID, contacts and any position they have stored on the radio.
 *
 * So: the folder says what is in it, and pruning is a button the operator
 * presses after reading how many files and how many bytes it would take.
 */
function RadioBackups() {
  const [summary, setSummary] = useState<RadioBackupsSummary | null>(null);
  const [keep, setKeep] = useState(5);
  const [pruning, setPruning] = useState(false);

  const load = useCallback(async (n: number) => {
    try {
      setSummary(await api.radioBackupsSummary(n));
    } catch {
      // A folder that cannot be read is not worth a toast on a settings screen
      // the operator opened for something else; the card simply says nothing.
      setSummary(null);
    }
  }, []);

  useEffect(() => {
    void load(keep);
  }, [load, keep]);

  const prunable = summary?.groups.reduce((n, g) => n + g.prunable_files, 0) ?? 0;
  const prunableBytes =
    summary?.groups.reduce((n, g) => n + g.prunable_bytes, 0) ?? 0;

  const doPrune = async () => {
    if (!summary || prunable === 0) return;
    const ok = await confirmDialog(
      `Delete ${prunable} backup file${prunable === 1 ? "" : "s"} ` +
        `(${humanBytes(prunableBytes)}), keeping the ${keep} most recent for each radio? ` +
        "A backup is how you put a radio back the way it was, and this cannot be undone.",
      { title: "Clean up radio backups", kind: "warning" },
    );
    if (!ok) return;
    setPruning(true);
    try {
      const res = await api.pruneRadioBackups(keep);
      toast.success(
        `Deleted ${res.files_deleted} file${res.files_deleted === 1 ? "" : "s"} — ` +
          `${humanBytes(res.bytes_freed)} freed`,
      );
      await load(keep);
    } catch (e) {
      toast.error(`Clean up failed: ${e}`);
    } finally {
      setPruning(false);
    }
  };

  return (
    <Card className="p-4">
      <h2 className="text-sm font-semibold text-slate-800 dark:text-slate-100">
        Radio backups
      </h2>
      <p className="mt-1 text-xs text-slate-500 dark:text-slate-400">
        Every time this app reads a radio or writes to one, it saves a copy of
        that radio's memory first — which is how you put the radio back the way
        it was. Each file is the radio's own flash, so it holds{" "}
        <strong>your call sign, DMR ID, contacts and any stored position</strong>
        , unencrypted. They are never deleted for you.
      </p>
      {summary === null ? (
        <p className="mt-3 text-xs text-slate-400">Nothing saved yet.</p>
      ) : (
        <>
          <p className="mt-2 break-all text-[11px] text-slate-400 dark:text-slate-500">
            <code className="rounded bg-slate-100 px-1 py-0.5 font-mono dark:bg-slate-700">
              {summary.dir}
            </code>
          </p>
          <p className="mt-3 text-xs text-slate-600 dark:text-slate-300">
            {summary.files} file{summary.files === 1 ? "" : "s"} ·{" "}
            {humanBytes(summary.bytes)}
          </p>
          {summary.groups.length > 0 && (
            <ul className="mt-2 space-y-1 text-[11px] text-slate-500 dark:text-slate-400">
              {summary.groups.map((g) => (
                <li key={g.key} className="flex flex-wrap items-baseline gap-x-2">
                  <span className="font-medium text-slate-600 dark:text-slate-300">
                    {g.label}
                  </span>
                  <span>
                    {g.files} file{g.files === 1 ? "" : "s"} ·{" "}
                    {humanBytes(g.bytes)}
                    {g.newest ? ` · newest ${g.newest}` : ""}
                  </span>
                  {g.prunable_files > 0 && (
                    <span className="text-amber-600 dark:text-amber-400">
                      {g.prunable_files} would be deleted
                    </span>
                  )}
                </li>
              ))}
            </ul>
          )}
          <div className="mt-3 flex flex-wrap items-center gap-2">
            <span className="text-xs text-slate-500 dark:text-slate-400">
              Keep the most recent
            </span>
            <Select
              className="w-16"
              value={String(keep)}
              onChange={(e) => setKeep(Number(e.target.value))}
            >
              {KEEP_CHOICES.map((n) => (
                <option key={n} value={n}>
                  {n}
                </option>
              ))}
            </Select>
            <span className="text-xs text-slate-500 dark:text-slate-400">
              per radio
            </span>
            <Button onClick={doPrune} disabled={pruning || prunable === 0}>
              {pruning ? <Spinner className="h-3.5 w-3.5" /> : <Eraser size={14} />}
              Clean up…
            </Button>
          </div>
          <p className="mt-2 text-[11px] text-slate-400 dark:text-slate-500">
            {prunable === 0
              ? "Nothing to clean up at that setting."
              : `Would delete ${prunable} file${prunable === 1 ? "" : "s"}, freeing ${humanBytes(prunableBytes)}.`}
          </p>
        </>
      )}
    </Card>
  );
}

export function Settings() {
  const { theme, setTheme } = useTheme();
  const [busy, setBusy] = useState<null | "backup" | "restore">(null);

  const doBackup = async () => {
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

          <RadioBackups />

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
