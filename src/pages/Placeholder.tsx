import type { ReactNode } from "react";
import { PageHeader, EmptyState } from "../components/ui";

/** Temporary scaffold for feature pages built out in later iterations. */
export function Placeholder({
  title,
  subtitle,
  icon,
  note,
}: {
  title: string;
  subtitle?: string;
  icon?: ReactNode;
  note?: string;
}) {
  return (
    <>
      <PageHeader title={title} subtitle={subtitle} />
      <div className="flex flex-1 items-center justify-center overflow-auto p-5">
        <EmptyState
          icon={icon}
          title={`${title} — coming together`}
          description={
            note ??
            "This section is part of the build. The backend commands are ready; the UI lands in the next iteration."
          }
        />
      </div>
    </>
  );
}
