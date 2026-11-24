// The staff pages' one row of tabs (components/look/StaffHead.astro): the
// queue's lists, then Reports, Reserved ids and the Log beside them, each
// with how many it holds where that's known: how many versions wait, how
// many reports are open.
import type { Edge } from "./edge.ts";
import type { QueueState } from "./types.ts";

export type StaffPage = Exclude<QueueState, "all"> | "reports" | "reservations" | "audit";

export interface StaffCounts {
  waiting: number | null;
  reports: number | null;
}

const LISTS: [Exclude<QueueState, "all">, string][] = [["undecided", "Waiting"], ["verified", "Verified"], ["declined", "Not verified"], ["taken_down", "Taken down"]];

export const queueHref = (state: QueueState) => (state === "undecided" ? "/admin/plugins" : `/admin/plugins?state=${state}`);

export function staffTabs(current: StaffPage | "all", counts: StaffCounts) {
  return [
    ...LISTS.map(([state, label]) => ({ label, href: queueHref(state), n: state === "undecided" ? counts.waiting : null, current: state === current })),
    { label: "Reports", href: "/admin/reports", n: counts.reports, current: current === "reports" },
    { label: "Reserved ids", href: "/admin/reservations", current: current === "reservations" },
    { label: "Log", href: "/admin/audit", current: current === "audit" },
  ];
}

/** The tabs' counts, asked of the edge (a page that has one of them already passes it). */
export async function staffCounts(edge: Edge, known: Partial<StaffCounts> = {}): Promise<StaffCounts> {
  const [waiting, reports] = await Promise.all([
    known.waiting !== undefined ? null : edge.queue("undecided"),
    known.reports !== undefined ? null : edge.reports("open"),
  ]);
  return {
    waiting: known.waiting !== undefined ? known.waiting : waiting?.ok ? waiting.data.total : null,
    reports: known.reports !== undefined ? known.reports : reports?.ok ? reports.data.total : null,
  };
}
