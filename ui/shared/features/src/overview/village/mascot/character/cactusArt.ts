// The cactus, measured from reference/cactus-rest.png (1320 × 2000, flat
// vector render on white) with the same colour-class scanline method as the
// monster. ART PIXELS of that image (x right, y down). Pure data.

export type P = [number, number]

/** origin: centre of the pot's base, on the ground */
export const CACTUS_ORIGIN: P = [659.5, 1405]
/** lab units per art pixel (about the monster's overall height) */
export const CACTUS_SCALE = 0.29

export const CACTUS_COLORS = {
  body: '#93BE44', // column
  shade: '#84B442', // the darker band down the column's right side
  arm: '#74A53E',
  spine: '#DDED8B',
  potRim: '#EB8D4F',
  pot: '#E2773C',
}

/**
 * Column: straight sides at x 450 / 869, an asymmetric rounded top (highest
 * ≈ y 199 left of centre), running down into the pot (hidden below y 1000).
 */
export const COLUMN: P[] = [
  [560, 1090],
  [450, 1090],
  [450, 1000],
  [450, 860],
  [450, 690],
  [450, 520],
  [452, 400],
  [453, 345],
  [458, 305],
  [472, 272],
  [492, 250],
  [515, 233],
  [548, 218],
  [600, 206],
  [650, 199],
  [700, 203],
  [745, 210],
  [785, 224],
  [815, 245],
  [840, 268],
  [857, 295],
  [866, 325],
  [869, 380],
  [869, 520],
  [869, 690],
  [869, 860],
  [869, 1000],
  [869, 1090],
  [760, 1090],
]

/** inner edge of the darker band (it runs from the top down to the pot) */
export const SHADE_EDGE: P[] = [
  [728, 190],
  [734, 240],
  [740, 280],
  [746, 320],
  [751, 360],
  [755, 400],
  [759, 440],
  [763, 500],
  [766, 580],
  [767, 640],
  [767, 760],
  [764, 880],
  [759, 960],
  [757, 1030],
]

/** Left arm: elbow out of the column's left side, forearm pointing up; base hidden behind the column (and run well in, so it never shows when the arm tips). */
export const ARM_L: P[] = [
  [530, 710],
  [430, 709],
  [392, 706],
  [341, 691],
  [311, 677],
  [290, 660],
  [272, 642],
  [258, 622],
  [245, 600],
  [235, 578],
  [228, 556],
  [222, 530],
  [218, 500],
  [216, 465],
  [218, 430],
  [225, 405],
  [236, 385],
  [255, 364],
  [280, 349],
  [305, 343],
  [330, 345],
  [352, 352],
  [372, 365],
  [388, 380],
  [398, 395],
  [404, 412],
  [408, 440],
  [413, 468],
  [419, 492],
  [428, 512],
  [436, 535],
  [446, 568],
  [452, 590],
  [530, 596],
]
export const ARM_L_PIVOT: P = [452, 655]
/** where a held tool would sit (the top of the forearm) */
export const ARM_L_TIP: P = [312, 372]

/** Right arm: the mirror pose, lower and a little bigger. */
export const ARM_R: P[] = [
  [790, 840],
  [863, 837],
  [900, 836],
  [956, 826],
  [992, 811],
  [1017, 796],
  [1035, 779],
  [1050, 763],
  [1062, 746],
  [1072, 728],
  [1081, 708],
  [1088, 690],
  [1094, 670],
  [1099, 648],
  [1102, 620],
  [1103, 590],
  [1101, 565],
  [1096, 542],
  [1088, 522],
  [1076, 504],
  [1060, 489],
  [1040, 477],
  [1015, 470],
  [990, 469],
  [965, 474],
  [945, 485],
  [930, 500],
  [920, 518],
  [915, 538],
  [911, 558],
  [907, 578],
  [902, 600],
  [896, 622],
  [889, 642],
  [879, 659],
  [866, 675],
  [790, 676],
]
export const ARM_R_PIVOT: P = [866, 755]
export const ARM_R_TIP: P = [1005, 500]

/** Spines: [cx, cy, length, width, angle° (image coords)], each a capsule on its part. */
export type Spine = [number, number, number, number, number]
export const SPINES_COLUMN: Spine[] = [
  [665, 196, 48, 26, -89],
  [709, 205, 48, 26, -73],
  [826, 262, 48, 26, -48],
  [460, 298, 48, 26, 43],
  [506, 820, 48, 26, 59],
  [786, 844, 48, 26, -59],
  [498, 917, 48, 26, 59],
  [806, 917, 48, 26, -59],
]
/** the one peeking out from behind the column's right shoulder */
export const SPINES_BEHIND: Spine[] = [[884, 340, 48, 26, -73]]
export const SPINES_ARM_L: Spine[] = [
  [309, 338, 48, 26, 83],
  [360, 360, 48, 26, -56],
  [213, 482, 48, 26, 59],
]
export const SPINES_ARM_R: Spine[] = [
  [968, 479, 48, 26, 72],
  [1017, 479, 48, 26, -74],
  [1099, 627, 48, 26, -31],
]

/**
 * Pot: a rim (378…941 × 1000…1117, r 24) over a tapering body whose sides
 * measure 414@1130 → 451@1370 and 905@1130 → 868@1370 (slope 0.154),
 * reaching the base (1405) at 456.5 / 862.5, bottom corners r 45.
 */
export const POT_RIM = { x0: 378, x1: 941, y0: 1000, y1: 1117, r: 24 }
// its top corners are square and tucked up under the rim (sides extended at the same slope)
export const POT_BODY: P[] = [
  [406, 1078],
  [913, 1078],
  [862.5, 1405],
  [456.5, 1405],
]
export const POT_BOTTOM = 1405

/**
 * The face is the monster's face unit at 0.717× (eye spacing 179 vs 246 px,
 * pupils 35 × 65 vs 47 × 91, smile 469 vs 677 wide): the monster's lobe
 * centre (740.5, 815) lands on (608.5, 523).
 */
export const FACE_FROM_MONSTER = {
  scale: 0.717,
  monster: [740.5, 815] as P,
  cactus: [608.5, 523] as P,
}
