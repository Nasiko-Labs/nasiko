// Ported from nasiko-ui-lab@1b99bdda common/src/components/shared/chart-marks.tsx. Local edits: the `Series` type is
// declared here as `SwatchSeries` (the two fields the swatch reads) instead of imported from the chart kit, because a
// composite may not depend on the lazy charts-kit entry (charts-stay-lazy); the kit's `Series` satisfies it.
/**
 * The legend key every chart draws the same way (DESIGN.md "Charts"): the series fill with its 2 px top
 * edge, matching the bars (the visx kit's `Bar`/`SeriesBar` `edge` prop draws the bar side).
 */
/** The series tokens the swatch draws: the chart kit's `Series` (`fill` for areas, `edge` for lines and the top edge). */
export interface SwatchSeries {
  fill: string;
  edge: string;
}

/** Legend key: the fill with its top edge (a line series passes `line` for a 2 px stroke in its edge). */
export function Swatch({ series, line }: { series: SwatchSeries; line?: boolean | undefined }) {
  if (line)
    return <span aria-hidden className="h-0.5 w-3 shrink-0" style={{ background: series.edge }} />;
  return (
    <span
      aria-hidden
      className="size-2.5 shrink-0 rounded-xs"
      style={{ background: series.fill, borderTop: `2px solid ${series.edge}` }}
    />
  );
}
