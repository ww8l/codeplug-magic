import { useCallback, useEffect, useState } from "react";
import {
  IdCard,
  RefreshCw,
  Download,
  Search,
  ChevronLeft,
  ChevronRight,
  X,
} from "lucide-react";
import { toast } from "sonner";
import { api, withToast } from "../lib/api";
import type { DmrUser, DmrUsersStatus } from "../lib/types";
import { PageHeader, Button, Spinner, EmptyState, TextInput, Select } from "../components/ui";
import { ExportDmrUsersDialog } from "../components/dmr-users/ExportDmrUsersDialog";

const PAGE_SIZE = 50;

function fmtCount(n: number) {
  return n.toLocaleString();
}

function fmtDate(iso: string | null) {
  if (!iso) return "never";
  // SQLite CURRENT_TIMESTAMP is UTC "YYYY-MM-DD HH:MM:SS" with no zone marker.
  const d = new Date(iso.replace(" ", "T") + "Z");
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleString();
}

export function DmrContacts() {
  const [status, setStatus] = useState<DmrUsersStatus | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [exporting, setExporting] = useState(false);

  const [search, setSearch] = useState("");
  const [countryFilter, setCountryFilter] = useState("");
  const [countries, setCountries] = useState<string[]>([]);
  const [page, setPage] = useState(0);
  const [rows, setRows] = useState<DmrUser[]>([]);
  const [loading, setLoading] = useState(true);

  const loadStatus = useCallback(async () => {
    setStatus(await api.dmrUsersStatus());
  }, []);

  const loadCountries = useCallback(async () => {
    setCountries(await api.listDmrUserCountries());
  }, []);

  const loadRows = useCallback(async () => {
    setLoading(true);
    try {
      setRows(
        await api.listDmrUsers(
          search.trim() || undefined,
          countryFilter || undefined,
          PAGE_SIZE,
          page * PAGE_SIZE,
        ),
      );
    } finally {
      setLoading(false);
    }
  }, [search, countryFilter, page]);

  useEffect(() => {
    loadStatus();
    loadCountries();
  }, [loadStatus, loadCountries]);

  useEffect(() => {
    loadRows();
  }, [loadRows]);

  // Any filter change resets to page 0.
  useEffect(() => {
    setPage(0);
  }, [search, countryFilter]);

  const handleRefresh = async () => {
    setRefreshing(true);
    const res = await withToast(api.refreshDmrUsers(), {
      error: "Could not refresh from RadioID.net",
    });
    setRefreshing(false);
    if (res) {
      toast.success(
        `Refreshed: ${fmtCount(res.fetched)} fetched, ${fmtCount(res.added)} new, ${fmtCount(res.updated)} updated (${fmtCount(res.total)} total)`,
      );
      loadStatus();
      loadCountries();
      loadRows();
    }
  };

  const hasFilters = !!(search || countryFilter);
  const clearFilters = () => {
    setSearch("");
    setCountryFilter("");
  };

  return (
    <>
      <PageHeader
        icon={<IdCard size={18} />}
        title="DMR Contacts"
        subtitle={
          status
            ? `${fmtCount(status.total)} contact${status.total === 1 ? "" : "s"} · last refreshed ${fmtDate(status.last_refreshed_at)}`
            : "…"
        }
        actions={
          <>
            <Button onClick={handleRefresh} disabled={refreshing}>
              {refreshing ? <Spinner className="h-3.5 w-3.5" /> : <RefreshCw size={14} />}
              Refresh from RadioID.net
            </Button>
            <Button
              variant="primary"
              onClick={() => setExporting(true)}
              disabled={!status || status.total === 0}
            >
              <Download size={14} /> Export…
            </Button>
          </>
        }
      />

      <div className="flex flex-wrap items-center gap-2 border-b border-slate-200 px-4 py-2 dark:border-slate-700">
        <div className="relative">
          <Search
            size={13}
            className="pointer-events-none absolute left-2 top-1/2 -translate-y-1/2 text-slate-400"
          />
          <TextInput
            className="w-64 pl-7"
            placeholder="Search callsign, name, DMR ID…"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
          />
        </div>
        <Select value={countryFilter} onChange={(e) => setCountryFilter(e.target.value)}>
          <option value="">All countries</option>
          {countries.map((c) => (
            <option key={c}>{c}</option>
          ))}
        </Select>
        {hasFilters && (
          <button
            onClick={clearFilters}
            className="inline-flex items-center gap-1 rounded-full bg-sky-100 px-2 py-0.5 text-[11px] font-medium text-sky-700 hover:bg-sky-200 dark:bg-sky-950 dark:text-sky-300"
          >
            Clear filters <X size={11} />
          </button>
        )}
      </div>

      {status && status.total === 0 ? (
        <div className="flex flex-1 items-center justify-center">
          <EmptyState
            icon={<IdCard size={40} strokeWidth={1.5} />}
            title="No DMR contacts yet"
            description="Pull the public DMR-ID database from RadioID.net to get started."
            action={
              <Button variant="primary" onClick={handleRefresh} disabled={refreshing}>
                <RefreshCw size={14} /> Refresh from RadioID.net
              </Button>
            }
          />
        </div>
      ) : loading ? (
        <div className="flex flex-1 items-center justify-center">
          <Spinner className="h-6 w-6" />
        </div>
      ) : rows.length === 0 ? (
        <div className="flex flex-1 items-center justify-center">
          <EmptyState
            icon={<Search size={40} strokeWidth={1.5} />}
            title="No matching contacts"
            description="No contacts match the current search/filter."
            action={
              <Button onClick={clearFilters}>
                <X size={14} /> Clear filters
              </Button>
            }
          />
        </div>
      ) : (
        <>
          <div className="min-h-0 flex-1 overflow-auto">
            <table className="w-full text-left text-xs">
              <thead className="sticky top-0 bg-slate-50 text-[11px] uppercase tracking-wide text-slate-400 dark:bg-slate-800">
                <tr>
                  <th className="px-5 py-2 font-semibold">DMR ID</th>
                  <th className="px-3 py-2 font-semibold">Callsign</th>
                  <th className="px-3 py-2 font-semibold">Name</th>
                  <th className="px-3 py-2 font-semibold">City</th>
                  <th className="px-3 py-2 font-semibold">State</th>
                  <th className="px-3 py-2 font-semibold">Country</th>
                </tr>
              </thead>
              <tbody>
                {rows.map((u) => (
                  <tr
                    key={u.id}
                    className="border-b border-slate-100 hover:bg-slate-50 dark:border-slate-700/60 dark:hover:bg-slate-800/40"
                  >
                    <td className="px-5 py-2 font-mono tabular-nums text-slate-700 dark:text-slate-200">
                      {u.dmr_id}
                    </td>
                    <td className="px-3 py-2 font-medium text-slate-800 dark:text-slate-100">
                      {u.callsign}
                    </td>
                    <td className="px-3 py-2 text-slate-600 dark:text-slate-300">
                      {[u.first_name, u.last_name].filter(Boolean).join(" ")}
                    </td>
                    <td className="px-3 py-2 text-slate-500">{u.city}</td>
                    <td className="px-3 py-2 text-slate-500">{u.state}</td>
                    <td className="px-3 py-2 text-slate-500">{u.country}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          <div className="flex items-center justify-between border-t border-slate-200 px-4 py-2 text-xs text-slate-500 dark:border-slate-700">
            <span>
              Showing {page * PAGE_SIZE + 1}–{page * PAGE_SIZE + rows.length}
            </span>
            <div className="flex gap-1.5">
              <Button onClick={() => setPage((p) => Math.max(0, p - 1))} disabled={page === 0}>
                <ChevronLeft size={13} /> Prev
              </Button>
              <Button
                onClick={() => setPage((p) => p + 1)}
                disabled={rows.length < PAGE_SIZE}
              >
                Next <ChevronRight size={13} />
              </Button>
            </div>
          </div>
        </>
      )}

      <ExportDmrUsersDialog open={exporting} onClose={() => setExporting(false)} />
    </>
  );
}
