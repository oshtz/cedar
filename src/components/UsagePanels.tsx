import type { ServiceHealth, UsagePanel } from "../types";

type UsagePanelsProps = {
  panels: UsagePanel[];
  health: ServiceHealth[];
};

export function UsagePanels({ panels, health }: UsagePanelsProps) {
  return (
    <section className="usage-column" aria-label="Usage and health panels">
      {panels.length === 0 && (
        <article className="panel usage-empty">
          <div className="panel-heading compact">
            <h2>Usage &amp; Health</h2>
            <span>0 services</span>
          </div>
          <MetricRow label="Requests" value="0" />
          <MetricRow label="Errors" value="0" />
          <MetricRow label="Storage" value="0 B" />
          <MetricRow label="Health" value="Not connected" />
        </article>
      )}

      {panels.map((panel) => (
        <article className={`panel usage-panel ${panel.tone}`} key={panel.id}>
          <div>
            <h3>{panel.title}</h3>
            <strong>{panel.value}</strong>
            <p>{panel.detail}</p>
          </div>
          <MiniTrend points={panel.points} tone={panel.tone} />
        </article>
      ))}

      <article className="panel health-panel">
        <div className="panel-heading compact">
          <h2>Service health</h2>
          <span>{health.filter((item) => item.status === "ok").length}/{health.length} ok</span>
        </div>
        <div className="health-list">
          {health.map((item) => (
            <div className="health-row" key={item.id}>
              <span className={`health-dot ${item.status}`} />
              <div>
                <strong>{item.service}</strong>
                <small>{item.detail}</small>
              </div>
              <em>{item.label}</em>
            </div>
          ))}
        </div>
      </article>
    </section>
  );
}

const chartWidth = 100;
const chartHeight = 100;
const chartPaddingX = 2;
const chartPaddingY = 8;

function normalizePoint(point: number, points: number[]) {
  const max = Math.max(...points, 1);
  const min = Math.min(...points, 0);
  const range = Math.max(max - min, 1);
  return Math.round(((point - min) / range) * 100);
}

function MetricRow({ label, value }: { label: string; value: string }) {
  return (
    <div className="usage-metric-row">
      <span>{label}</span>
      <strong>{value}</strong>
      <i />
    </div>
  );
}

function MiniTrend({ points, tone }: { points: number[]; tone: UsagePanel["tone"] }) {
  if (points.length === 0) return <div className="sparkline empty" aria-hidden="true" />;

  return (
    <div className="sparkline" aria-hidden="true">
      <MiniTrendChart points={points} tone={tone} />
    </div>
  );
}

function MiniTrendChart({ points, tone }: { points: number[]; tone: UsagePanel["tone"] }) {
  const pathPoints = seriesPoints(points.map((point) => normalizePoint(point, points)));
  const color = tone === "good" ? "#14a37a" : tone === "warn" || tone === "bad" ? "#d85b2c" : "#ff6d3a";

  if (pathPoints.length === 0) return null;

  return (
    <svg className="mini-trend-chart" aria-hidden="true" viewBox={`0 0 ${chartWidth} ${chartHeight}`} preserveAspectRatio="none">
      <path d={linePath(pathPoints)} fill="none" stroke={color} strokeLinecap="round" strokeLinejoin="round" strokeWidth="4" />
    </svg>
  );
}

function seriesPoints(values: number[]) {
  return values.map((value, index) => {
    const x = values.length === 1 ? chartWidth / 2 : chartPaddingX + (index / (values.length - 1)) * (chartWidth - chartPaddingX * 2);
    const y = chartHeight - chartPaddingY - (clamp(value, 0, 100) / 100) * (chartHeight - chartPaddingY * 2);
    return [round(x), round(y)] as const;
  });
}

function linePath(points: Array<readonly [number, number]>) {
  return points.map(([x, y], index) => `${index === 0 ? "M" : "L"} ${x} ${y}`).join(" ");
}

function clamp(value: number, min: number, max: number) {
  return Math.min(Math.max(value, min), max);
}

function round(value: number) {
  return Math.round(value * 100) / 100;
}
