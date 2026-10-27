// The monster as data, measured from reference/monster-rest.png (1818 × 2000,
// a flat vector render on pure white). No SVG was supplied, so every number
// here comes from colour-class scanline and column profiles of that PNG
// (see README → References). Coordinates are ART PIXELS of that image:
// x right, y down. Pure data: nothing is evaluated.
//
// Each outline is a list of on-curve points, in order around the shape; the
// puppet turns them into a smooth closed chain of cubic Béziers (centripetal
// Catmull-Rom), then deforms the CONTROL POINTS, so silhouettes stay smooth
// at any size. Parts that are partly hidden in the art (the body under the
// arm and ears, the arms behind the body, the feet's tops) are completed
// with plausible curves that stay hidden at rest.

export type P = [number, number]

/** origin of the lab's coordinates: between the feet, on the ground */
export const ART_ORIGIN: P = [869, 1778]
/** lab units per art pixel (the body is ~300 units wide) */
export const ART_SCALE = 0.25

/** Exact hex, sampled from the render (mode of each colour region). */
export const ART_COLORS = {
  body: '#F3AC3D',
  dark: '#E68927', // far arm, arm crease, belly line, left spots
  spotLight: '#EA972F', // right spots
  face: '#F8C170',
  toe: '#FEDFAF',
  leaf: '#7FB951',
  leafLight: '#D2F29A',
  eyeWhite: '#FFFFFF',
  ink: '#000000', // pupils and mouth
  stage: '#FFFFFF',
}

/**
 * Main body blob, clockwise from the top of the dome. The ears, arms and
 * feet are separate parts; the outline runs under them (hidden at rest).
 * Measured silhouette: dome top (810, 523); left edge 521@600, 489@680,
 * 416@800, 368@900, 332@1000, 309@1100, 300@1180, 302@1300 (from here the
 * far arm is behind it), 318@1400, 348@1500, 397@1600; right edge 1135@618
 * (just under the right ear), 1238@700, 1317@800, 1370@900, 1406@1000,
 * 1436@1100 (from here the near arm covers it; mirrored below).
 */
export const BODY: P[] = [
  [810, 523],
  [891, 531],
  [971, 547],
  [1023, 566],
  [1080, 589],
  [1135, 618],
  [1189, 655],
  [1238, 700],
  [1290, 760],
  [1340, 840],
  [1378, 920],
  [1406, 1000],
  [1425, 1060],
  [1440, 1140],
  [1444, 1240],
  [1436, 1330],
  [1415, 1420],
  [1385, 1500],
  [1340, 1580],
  [1282, 1638],
  [1160, 1672],
  [1000, 1684],
  [870, 1686],
  [778, 1684],
  [680, 1680],
  [560, 1672],
  [470, 1656],
  [423, 1640],
  [385, 1580],
  [348, 1500],
  [318, 1400],
  [302, 1300],
  [298, 1220],
  [301, 1160],
  [309, 1100],
  [321, 1040],
  [338, 980],
  [360, 920],
  [386, 860],
  [416, 800],
  [449, 740],
  [489, 680],
  [509, 640],
  [521, 600],
  [540, 578],
  [590, 568],
  [624, 567],
  [664, 547],
  [732, 531],
]

/**
 * The body with the ears grown into it (one outline, so they can never look
 * detached). Ear points carry a weight (0 at the base → 1 at the tip) used to
 * flick and resize them.
 */
export const BODY_EARS: [number, number, number][] = [
  [810, 523, 0],
  [891, 531, 0],
  [971, 547, 0],
  [1023, 566, 0],
  [1041, 547, 0.4],
  [1060, 532, 0.75],
  [1081, 526, 1],
  [1096, 533, 0.85],
  [1105, 547, 0.6],
  [1113, 565, 0.35],
  [1123, 590, 0.12],
  [1135, 618, 0],
  [1189, 655, 0],
  [1238, 700, 0],
  [1290, 760, 0],
  [1340, 840, 0],
  [1378, 920, 0],
  [1406, 1000, 0],
  [1425, 1060, 0],
  [1440, 1140, 0],
  [1444, 1240, 0],
  [1436, 1330, 0],
  [1415, 1420, 0],
  [1385, 1500, 0],
  [1340, 1580, 0],
  [1282, 1638, 0],
  [1160, 1672, 0],
  [1000, 1684, 0],
  [870, 1686, 0],
  [778, 1684, 0],
  [680, 1680, 0],
  [560, 1672, 0],
  [470, 1656, 0],
  [423, 1640, 0],
  [385, 1580, 0],
  [348, 1500, 0],
  [318, 1400, 0],
  [302, 1300, 0],
  [298, 1220, 0],
  [301, 1160, 0],
  [309, 1100, 0],
  [321, 1040, 0],
  [338, 980, 0],
  [360, 920, 0],
  [386, 860, 0],
  [416, 800, 0],
  [449, 740, 0],
  [489, 680, 0],
  [509, 640, 0],
  [521, 600, 0],
  [529, 580, 0.15],
  [537, 560, 0.45],
  [548, 542, 0.75],
  [567, 525, 1],
  [586, 531, 0.85],
  [604, 544, 0.6],
  [618, 558, 0.3],
  [626, 568, 0],
  [664, 547, 0],
  [732, 531, 0],
]

/** Pointed ears on the top corners, drawn behind the body (same colour). Tips (567, 525) and (1081, 526). */
export const EAR_L: P[] = [
  [528, 600],
  [529, 583],
  [536, 563],
  [546, 545],
  [556, 532],
  [567, 525],
  [584, 530],
  [602, 542],
  [617, 557],
  [626, 572],
  [630, 600],
  [580, 616],
]
export const EAR_R: P[] = [
  [1020, 578],
  [1027, 563],
  [1041, 547],
  [1060, 532],
  [1081, 526],
  [1094, 532],
  [1103, 545],
  [1111, 560],
  [1119, 579],
  [1127, 599],
  [1136, 620],
  [1120, 642],
  [1062, 622],
]
export const EAR_PIVOT_L: P = [578, 590]
export const EAR_PIVOT_R: P = [1080, 592]

/**
 * Feet: two lobes under the body with a notch between them at (778, 1689).
 * The near (right) foot is wider and lower (bottom 1778) than the far one
 * (bottom 1767): the art is a slight ¾ view.
 */
export const FOOT_L: P[] = [
  [437, 1660],
  [454, 1680],
  [473, 1700],
  [494, 1720],
  [516, 1740],
  [545, 1756],
  [580, 1765],
  [606, 1767],
  [639, 1764],
  [665, 1756],
  [694, 1740],
  [718, 1720],
  [746, 1700],
  [778, 1689],
  [770, 1640],
  [600, 1605],
  [460, 1620],
]
export const FOOT_R: P[] = [
  [782, 1690],
  [811, 1700],
  [836, 1712],
  [867, 1732],
  [904, 1756],
  [948, 1772],
  [1008, 1778],
  [1047, 1776],
  [1086, 1768],
  [1125, 1756],
  [1150, 1740],
  [1178, 1720],
  [1200, 1700],
  [1219, 1680],
  [1236, 1660],
  [1262, 1640],
  [1240, 1600],
  [1000, 1610],
  [800, 1645],
]
/** foot centres (used for lifting a foot) */
export const FOOT_PIVOT_L: P = [606, 1720]
export const FOOT_PIVOT_R: P = [1010, 1725]

/**
 * Toe pads: pale pointed arches whose lower part is cut by the foot
 * outline. [centre x, apex y, half-width, apex → widest]. Measured:
 * 492–546 apex 1694; 559–627 apex 1716; 1001–1065 apex 1729; 1080–1143 apex 1712.
 */
export const TOES_L: [number, number, number, number][] = [
  [519, 1694, 27, 22],
  [594, 1716, 34, 42],
]
export const TOES_R: [number, number, number, number][] = [
  [1034, 1729, 33, 40],
  [1111, 1712, 32, 32],
]

/**
 * Near (right) arm, a big mitten hanging down the right side, in front of
 * the body. Its inner edge is the dark crease; its outer edge merges into the
 * silhouette (1461@1240, 1482@1390, 1464@1500, 1421@1580, 1339@1640). The top
 * (shoulder cap) is hidden in the body and is round about the pivot, so the
 * arm can swing without changing the silhouette at the shoulder.
 */
export const ARM_R: P[] = [
  [1249, 1150],
  [1247, 1200],
  [1238, 1260],
  [1222, 1320],
  [1206, 1380],
  [1194, 1440],
  [1186, 1500],
  [1188, 1560],
  [1200, 1595],
  [1218, 1620],
  [1250, 1640],
  [1292, 1648],
  [1339, 1641],
  [1380, 1620],
  [1421, 1580],
  [1447, 1540],
  [1464, 1500],
  [1478, 1440],
  [1482, 1390],
  [1472, 1300],
  [1461, 1240],
  // shoulder cap: a circle of r 102 about the pivot, so the arm swings without
  // its top ever poking out of the body (the join stays one smooth shape)
  [1449, 1180],
  [1433, 1139],
  [1396, 1102],
  [1345, 1088],
  [1294, 1102],
  [1261, 1131],
]
export const ARM_PIVOT_R: P = [1345, 1190]
/** where a held prop sits in the near hand (art px, in the arm's rest frame) */
export const HAND_R: P = [1330, 1585]
/**
 * The far (left) arm's pivot, hand and crease are the near arm's, mirrored
 * about the body's centre line (x = 869) and moved 14 px inward, 2 px up.
 */
export const ARM_L_SHIFT: P = [14, -2]
/**
 * Its outline is measured, not mirrored: the visible outer edge is fuller at
 * the bottom than the near arm's (294@1200, 276@1300, 272@1400, 283@1500,
 * 300@1550, 332@1600, 355@1620, 395@1640, bottom ≈ 1648). The inner edge is
 * the near arm's crease mirrored (hidden behind the body at rest).
 */
export const ARM_L: P[] = [
  [503, 1148],
  [505, 1198],
  [514, 1258],
  [530, 1318],
  [546, 1378],
  [558, 1438],
  [566, 1498],
  [564, 1558],
  [552, 1593],
  [534, 1618],
  [500, 1640],
  [450, 1650],
  [408, 1648],
  [374, 1634],
  [347, 1615],
  [324, 1592],
  [306, 1566],
  [293, 1536],
  [283, 1500],
  [276, 1450],
  [272, 1400],
  [273, 1350],
  [277, 1300],
  [285, 1250],
  [294, 1200],
  [305, 1150],
  [319, 1137],
  [356, 1100],
  [407, 1086],
  [458, 1100],
  [491, 1129],
]

/**
 * The arm crease: a tapered dark line along the near arm's inner edge.
 * Least-squares cubic fit to the measured centre line (max error 1.2 px);
 * width 3 → 11 → 4 px.
 */
export const CREASE: { p: P[]; w: [number, number, number] } = {
  p: [
    [1249.5, 1150],
    [1245.2, 1291.3],
    [1131.5, 1479.2],
    [1213, 1612],
  ],
  w: [3, 11, 4],
}
/** The belly line between the feet (fit error 0.7 px), width 1 → 5 → 1 px. */
export const BELLY: { p: P[]; w: [number, number, number] } = {
  p: [
    [645, 1670],
    [736.3, 1692.6],
    [841.6, 1695.6],
    [935, 1686],
  ],
  w: [1.2, 5.5, 1.2],
}

/** Spots: [cx, cy, rx, ry]. Left cluster in the dark tone, right cluster lighter. */
export const SPOTS_L: [number, number, number, number][] = [
  [389, 1170, 22, 21],
  [404, 1251, 42, 40],
  [476, 1226, 15, 16],
]
export const SPOTS_R: [number, number, number, number][] = [
  [1050, 1452, 22, 20],
  [957, 1497, 20, 16],
  [1027, 1528, 43, 41],
  [1101, 1544, 15, 16],
]

/**
 * The lighter face patch: a rounded blob from (838, 623) at the top to
 * y ≈ 1107 at the bottom, widest at x 464…1225. Its top-left runs under the
 * eye mask (hidden), so the left eye lobe pokes out past it onto the body.
 */
export const FACE: P[] = [
  [838, 623],
  [915, 630],
  [991, 650],
  [1038, 670],
  [1075, 690],
  [1118, 720],
  [1151, 750],
  [1176, 780],
  [1196, 810],
  [1210, 840],
  [1220, 870],
  [1225, 910],
  [1223, 955],
  [1214, 982],
  [1197, 1010],
  [1168, 1040],
  [1137, 1060],
  [1087, 1080],
  [1049, 1090],
  [991, 1100],
  [900, 1106],
  [815, 1107],
  [700, 1103],
  [640, 1099],
  [585, 1090],
  [553, 1080],
  [514, 1060],
  [490, 1040],
  [477, 1020],
  [469, 1000],
  [464, 970],
  [466, 940],
  [470, 910],
  [476, 870],
  [486, 838],
  [505, 800],
  [540, 750],
  [585, 705],
  [640, 670],
  [700, 643],
  [770, 627],
]

/**
 * Eye mask: the union of two circles of radius 123 at (624, 815) and
 * (857, 815) (fitted to the top profile within 2 px), with straight sides
 * at x 501 and 980 running down to the smile, which forms its bottom edge.
 */
export const EYE = { cL: [624, 815] as P, cR: [857, 815] as P, r: 123 }
/**
 * Pupils: vertical pills 47 px wide, 91 px tall (centres (571, 835.5) and
 * (817, 835.5)) — looking slightly left and down within their lobes.
 */
export const PUPIL = { w: 47, half: 22, l: [571, 835.5] as P, r: [817, 835.5] as P }

/**
 * Two small fangs standing on the smile just inside its corners
 * (reference/monster-teeth.png, same frame as the rest art): a straight inner
 * side (x 461 / 1028), a rounded tip (top 825 / 828) and an outer side sloping
 * down to the line (meets it at 424 / 1067). Their bottoms run on under the
 * smile, which draws over them.
 */
export const TOOTH_L: P[] = [
  [461, 884],
  [461, 836],
  [457, 826],
  [450, 826],
  [443, 832],
  [433, 844],
  [424, 858],
  [416, 872],
]
export const TOOTH_R: P[] = [
  [1028, 888],
  [1028, 837],
  [1031, 829],
  [1037, 828],
  [1043, 832],
  [1052, 842],
  [1061, 854],
  [1068, 865],
  [1074, 878],
]

/** The smile: a 16 px black line; least-squares cubic fit (max error 2.3 px). */
export const MOUTH = {
  p: [
    [403, 857],
    [603.6, 984.5],
    [876.5, 989.9],
    [1080, 868],
  ] as P[],
  w: 16,
}

/**
 * Leaves on two stems, drawn behind the head. The stems run on down into the
 * head (hidden), so the leaves always look rooted. Big dark leaf: blade 524…838,
 * top 327; its tail is the dark stem (805…834). Small light leaf: 836…1034,
 * top 369, on its own thin stem (834…847).
 */
export const LEAF_BIG: P[] = [
  [806, 585],
  [806, 530],
  [806, 511],
  [801, 495],
  [788, 485],
  [770, 480],
  [757, 479],
  [731, 482],
  [697, 486],
  [660, 486],
  [625, 482],
  [600, 476],
  [575, 465],
  [552, 449],
  [536, 433],
  [527, 415],
  [525, 397],
  [528, 380],
  [537, 366],
  [553, 351],
  [580, 337],
  [612, 329],
  [650, 327],
  [693, 331],
  [736, 341],
  [770, 357],
  [797, 377],
  [814, 397],
  [826, 418],
  [834, 440],
  [837, 465],
  [837, 490],
  [834, 511],
  [833, 530],
  [833, 585],
]
export const LEAF_SMALL: P[] = [
  [835, 585],
  [835, 530],
  [836, 511],
  [838, 491],
  [838, 470],
  [836, 450],
  [839, 434],
  [848, 418],
  [862, 402],
  [882, 388],
  [907, 377],
  [935, 371],
  [956, 369],
  [977, 371],
  [998, 377],
  [1015, 388],
  [1027, 402],
  [1033, 418],
  [1034, 433],
  [1029, 450],
  [1018, 466],
  [1002, 478],
  [984, 487],
  [960, 493],
  [930, 494],
  [902, 491],
  [884, 488],
  [866, 492],
  [854, 499],
  [848, 511],
  [847, 530],
  [847, 585],
]
// pivots sit inside the head, so a swaying stem stays buried at its base
export const LEAF_PIVOT_BIG: P = [819, 566]
export const LEAF_PIVOT_SMALL: P = [841, 566]
