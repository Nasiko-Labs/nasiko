'use client';

export function ChartTooltip({
  active,
  payload,
  label,
  unit = '',
  accent = '#4facfe',
  formatter,
}: {
  active?: boolean;
  payload?: { value: number; name?: string }[];
  label?: string;
  unit?: string;
  accent?: string;
  formatter?: (value: number) => string;
}) {
  if (!active || !payload?.length) return null;
  const raw = payload[0].value;
  const display = formatter ? formatter(raw) : `${raw}${unit ? ` ${unit}` : ''}`;
  return (
    <div
      className="rounded-xl px-3 py-2 text-xs"
      style={{
        background: 'rgba(13,20,64,0.92)',
        backdropFilter: 'blur(14px)',
        boxShadow: `0 12px 32px rgba(0,0,0,0.45), 0 0 0 1px ${accent}33`,
      }}
    >
      <p
        className="mb-0.5 text-[10px] font-medium uppercase tracking-wider"
        style={{ color: 'rgba(255,255,255,0.5)' }}
      >
        {label}
      </p>
      <p className="text-sm font-bold" style={{ color: accent }}>
        {display}
      </p>
    </div>
  );
}
