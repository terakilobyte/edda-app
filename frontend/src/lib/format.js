// Number and timestamp formatting shared by the panels. Pure; no IPC.

export const fmtInt = (n) => (n == null ? "—" : Math.round(n).toLocaleString());
export const fmtCr = (n) => (n == null ? "—" : `${Math.round(n).toLocaleString()} cr`);
export const fmtCrShort = (n) => {
  if (n == null) return "—";
  const a = Math.abs(n);
  if (a >= 1e9) return `${(n / 1e9).toFixed(2)}B cr`;
  if (a >= 1e6) return `${(n / 1e6).toFixed(1)}M cr`;
  if (a >= 1e3) return `${(n / 1e3).toFixed(0)}k cr`;
  return `${n} cr`;
};
export const fmtLy = (n) => (n == null ? "—" : `${n.toFixed(1)} ly`);
export const fmtLs = (n) => (n == null ? "—" : `${Math.round(n).toLocaleString()} ls`);
export const fmtMin = (seconds) => (seconds == null ? "—" : `${Math.round(seconds / 60)} min`);
export const fmtAge = (hours) => {
  if (hours == null || !isFinite(hours)) return "unknown";
  if (hours < 1) return `${Math.round(hours * 60)} min`;
  if (hours < 48) return `${hours.toFixed(0)} h`;
  return `${(hours / 24).toFixed(0)} d`;
};
export const fmtTs = (ts) => (ts ? ts.replace("T", " ").replace("Z", "") : "");
