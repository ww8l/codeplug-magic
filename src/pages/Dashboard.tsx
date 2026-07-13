import { useEffect, useState } from "react";
import { useNavigate } from "react-router-dom";
import { Radio, List, Package, ScanLine, Users, Upload, Plus, LayoutDashboard } from "lucide-react";
import { api } from "../lib/api";
import type { CodeplugSummary, DashboardStats } from "../lib/types";
import { Button, Card, PageHeader, Spinner } from "../components/ui";
import { HandheldRadio } from "../components/icons/HandheldRadio";

const STAT_CARDS = [
  { key: "total_channels", label: "Channels", icon: Radio },
  { key: "total_channel_lists", label: "Zones/Groups", icon: List },
  { key: "total_scan_lists", label: "Scan Lists", icon: ScanLine },
  { key: "total_talkgroups", label: "Talkgroups", icon: Users },
  { key: "total_codeplugs", label: "Codeplugs", icon: Package },
  { key: "total_radio_profiles", label: "Radios", icon: HandheldRadio },
] as const;

export function Dashboard() {
  const navigate = useNavigate();
  const [stats, setStats] = useState<DashboardStats | null>(null);
  const [recent, setRecent] = useState<CodeplugSummary[]>([]);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    Promise.all([api.getDashboardStats(), api.getRecentCodeplugs()])
      .then(([s, r]) => {
        setStats(s);
        setRecent(r);
      })
      .finally(() => setLoading(false));
  }, []);

  return (
    <>
      <PageHeader icon={<LayoutDashboard size={18} />} title="Dashboard" subtitle="Overview of your master database" />
      <div className="flex-1 overflow-auto p-5">
        {loading ? (
          <div className="flex justify-center py-20">
            <Spinner className="h-6 w-6" />
          </div>
        ) : (
          <div className="space-y-6">
            {/* Stat cards */}
            <div className="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-6">
              {STAT_CARDS.map(({ key, label, icon: Icon }) => (
                <Card key={key} className="p-4">
                  <div className="flex items-center justify-between">
                    <span className="text-xs font-medium text-slate-500 dark:text-slate-400">
                      {label}
                    </span>
                    <Icon size={16} className="text-slate-400" />
                  </div>
                  <div className="mt-2 text-2xl font-semibold tabular-nums text-slate-900 dark:text-slate-100">
                    {stats ? stats[key] : 0}
                  </div>
                </Card>
              ))}
            </div>

            {/* Quick actions */}
            <div>
              <h2 className="mb-2 text-xs font-semibold uppercase tracking-wide text-slate-400">
                Quick Actions
              </h2>
              <div className="flex gap-2">
                <Button variant="primary" onClick={() => navigate("/channels?import=1")}>
                  <Upload size={14} /> Import CSV
                </Button>
                <Button onClick={() => navigate("/codeplugs?new=1")}>
                  <Plus size={14} /> New Codeplug
                </Button>
              </div>
            </div>

            {/* Recent codeplugs */}
            <div>
              <h2 className="mb-2 text-xs font-semibold uppercase tracking-wide text-slate-400">
                Recent Codeplugs
              </h2>
              <Card>
                {recent.length === 0 ? (
                  <div className="px-4 py-8 text-center text-xs text-slate-400">
                    No codeplugs yet. Create one to get started, e.g.
                    "W0CPH-AT878-Truck".
                  </div>
                ) : (
                  <ul className="divide-y divide-slate-100 dark:divide-slate-700">
                    {recent.map((cp) => (
                      <li
                        key={cp.id}
                        onClick={() => navigate(`/codeplugs/${cp.id}`)}
                        className="flex cursor-pointer items-center justify-between px-4 py-2.5 hover:bg-slate-50 dark:hover:bg-slate-700/50"
                      >
                        <div>
                          <div className="text-[13px] font-medium text-slate-800 dark:text-slate-100">
                            {cp.name}
                          </div>
                          <div className="text-xs text-slate-500 dark:text-slate-400">
                            {cp.model_display_name ?? "No radio profile"} ·{" "}
                            {cp.channel_count} channels
                          </div>
                        </div>
                        <div className="text-xs text-slate-400">
                          {cp.last_exported
                            ? `${
                                cp.last_export_kind === "radio"
                                  ? "Programmed"
                                  : "Exported"
                              } ${cp.last_exported.slice(0, 10)}`
                            : "Never exported"}
                        </div>
                      </li>
                    ))}
                  </ul>
                )}
              </Card>
            </div>
          </div>
        )}
      </div>
    </>
  );
}
