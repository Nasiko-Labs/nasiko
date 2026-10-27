/**
 * The village's scenery (plans/feat-village-overview.md V50), where the reference design has it: trees and bushes
 * round the battery and between the barn and the clock tower, sorted in with the buildings but never selectable, and
 * grass tufts on the ground. Data only.
 */

/** Trees: [i, j, scale]; 1 is the reference's big tree. */
export const TREES: readonly (readonly [number, number, number])[] = [
  [0.35, 3.75, 0.8],
  [5.1, 0.4, 0.9],
  [4.5, 0.95, 0.42],
  [5.4, 5.24, 0.66],
  [18.5, 11.06, 1],
  [18.55, 11.55, 0.42],
]
/** Bushes: [i, j, scale]. */
export const BUSHES: readonly (readonly [number, number, number])[] = [
  [5.5, 2.9, 0.8],
  [14.9, 13.3, 0.9],
  [15.75, 13.25, 0.85],
  [8.8, 17.1, 0.9],
]
/** Grass tufts: [i, j]. */
export const TUFTS: readonly (readonly [number, number])[] = [
  [0.9, 10.2],
  [3.6, 10.3],
  [5.6, 9.9],
  [8.2, 10.4],
  [0.8, 14.6],
  [3.2, 14.4],
  [7.5, 14.1],
  [2.8, 17.4],
  [6.2, 17.6],
  [9.6, 11.4],
  [10.2, 10.4],
  [13.3, 10.6],
  [12.8, 4.6],
  [16.2, 4.3],
  [9.4, 3],
  [5.6, 3.4],
  [7.3, 8.3],
  [15.6, 10.6],
  [18.2, 4.4],
  [18.2, 10.2],
]
