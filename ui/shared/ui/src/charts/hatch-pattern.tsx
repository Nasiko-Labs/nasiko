/**
 * Lab: a diagonal hatch for "not recorded" bar slots (plans/feat-optimization-page.md R5A). The kit hoists any child
 * whose name contains "Pattern" into the chart's svg (`chart-defs.ts` `isPatternDefComponent`), so a Bar can fill with
 * `url(#id)`. Theme tokens only; static, so nothing moves under reduced motion.
 */
export function HatchPattern({ id, stroke = 'var(--border)' }: { id: string; stroke?: string }) {
  return (
    <defs>
      <pattern
        id={id}
        width={6}
        height={6}
        patternUnits="userSpaceOnUse"
        patternTransform="rotate(45)"
      >
        <line x1={0} y1={0} x2={0} y2={6} stroke={stroke} strokeWidth={2} />
      </pattern>
    </defs>
  )
}
