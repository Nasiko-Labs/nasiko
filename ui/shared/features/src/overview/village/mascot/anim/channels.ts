// Shared numeric pose representation. Every engine drives these same channels;
// the SVG renderer derives the deformed puppet, the face and props from them.
// All channels are offsets from the saved design, so 0 everywhere = the
// design at rest (the art). Layers are summed, then clamped by the renderer.

export const GLYPH_IDS = [
  'happy',
  'closed',
  'squeeze',
  'dot',
  'wide',
  'line',
  'half',
  'slant',
  'scowl',
] as const
export type GlyphId = (typeof GLYPH_IDS)[number]
export const GLYPH_CHANNELS = GLYPH_IDS.map((g) => `g_${g}` as const)
export type GlyphChannel = `g_${GlyphId}`

export const CHANNELS = [
  // whole body
  'sx', // width offset (0 = 1×); height changes are volume-preserving
  'sy', // height offset, about the ground
  'lean', // deg, shears the body over its planted feet (+ = leans right)
  'lift', // fraction of the body height, + = up (hops)
  'turn', // −1..1 body turn (+ = turns to the viewer's right)
  'tip', // −1..1 face tips (+ = looks up)
  'bend', // curl toward a motion: top shifts sideways (fraction of height)
  'bulge', // sides bow out (+) or pinch (−), mid-body
  'breath', // 0..1 belly expansion
  'breathShift', // −1..1 breathing redistribution (belly ↔ chest)
  'jx', // secondary follow-through bend (written by the shared controller)
  'jy', // secondary vertical squash (shared controller)
  // head (the upper body leads)
  'headTurn', // −1..1 added to the turn above the belly
  'headTip', // −1..1
  'headTilt', // deg, rolls the head over the body (+ = clockwise)
  'headLag', // turn lag written by the controller (overlapping action)
  // limbs (deg; + = outward and up for either arm)
  'armL',
  'armR',
  'armLFwd', // 0..1 brings the arm round to the front
  'armRFwd',
  'armLLen', // reach offset (+ = longer)
  'armRLen',
  'earL', // deg, + = outward
  'earR',
  'leaf', // deg, + = clockwise
  'footL', // lift, fraction of body height
  'footR',
  'footSpread', // feet slide apart (+) / together, fraction of width
  'soles', // 0..1 sitting: the soles turn to face us (toe pads showing)
  // secondary springs (written by the shared controller)
  'sArmL',
  'sArmR',
  'sEarL',
  'sEarR',
  'sLeaf',
  // ambient drift of parts (−1..1, scaled by the drift setting)
  'dTurn',
  'dTilt',
  'dEar',
  'dLeaf',
  'dArm',
  'dBob',
  // face: pupil glyph weights (absolute; the pill fills whatever is left)
  ...GLYPH_CHANNELS,
  'pupil', // pupil size offset
  'gazeX',
  'gazeY', // −1..1
  'blink', // 0..1
  'shutL', // 0..1 one eye closed (winks)
  'shutR',
  'lidL', // 0..1 upper lid lowered over the eye mask
  'lidR',
  'lidTiltL', // deg, + = inner corner up (worried); − would be angry
  'lidTiltR',
  'mouthSmile', // + deeper smile, − flatter / frown
  'mouthEnds', // + corners up, − corners down
  'mouthOpen', // 0..1
  'mouthWave', // S-curve (nervous)
  'mouthTilt', // smirk: + right corner up
  'mouthW', // width offset
  'blush', // 0..1
  // effects (appearance 0..1) and their shared phase
  'fxMarks',
  'fxSweat',
  'fxQuestion',
  'fxBulb',
  'fxSparkle',
  'fxPhase', // 0..1 looping phase for floating effects
  // props
  'cap',
  'blueprint',
  'wrench',
  'glass',
  'tiles',
  'wrenchTurn', // −1..1
  'glassDrift', // −1..1
  'tilesPhase', // 0..12 tile reorder phase
  'propBob', // −1..1 shared floating bob
  'propSway', // −1..1 second, faster rhythm
] as const

export type Channel = (typeof CHANNELS)[number]
export type Pose = Record<Channel, number>

export function zeroPose(): Pose {
  const p = {} as Pose
  for (const c of CHANNELS) p[c] = 0
  return p
}

export function copyPose(src: Partial<Pose>, dst: Pose = zeroPose()): Pose {
  for (const c of CHANNELS) dst[c] = src[c] ?? 0
  return dst
}

export const BODY_CHANNELS: Channel[] = [
  'sx',
  'sy',
  'lean',
  'lift',
  'turn',
  'tip',
  'bend',
  'bulge',
  'headTurn',
  'headTip',
  'headTilt',
  'armL',
  'armR',
  'armLFwd',
  'armRFwd',
  'armLLen',
  'armRLen',
  'earL',
  'earR',
  'leaf',
  'footL',
  'footR',
  'footSpread',
  'soles',
]
export const FACE_CHANNELS: Channel[] = [
  ...GLYPH_CHANNELS,
  'pupil',
  'gazeX',
  'gazeY',
  'shutL',
  'shutR',
  'lidL',
  'lidR',
  'lidTiltL',
  'lidTiltR',
  'mouthSmile',
  'mouthEnds',
  'mouthOpen',
  'mouthWave',
  'mouthTilt',
  'mouthW',
  'blush',
]
export const PROP_APPEAR: Channel[] = ['cap', 'blueprint', 'wrench', 'glass', 'tiles']
export const FX_APPEAR: Channel[] = ['fxMarks', 'fxSweat', 'fxQuestion', 'fxBulb', 'fxSparkle']
export const GESTURE_CHANNELS: Channel[] = [
  'wrenchTurn',
  'glassDrift',
  'tilesPhase',
  'propBob',
  'propSway',
  'fxPhase',
]

/**
 * Generators a clip can take ownership of. While a clip owns a generator, that
 * generator's layer is excluded from composition and its scheduler idles, so
 * exactly one source drives those channels.
 */
export type Generator = 'blink' | 'breath' | 'gaze' | 'activity' | 'bounce'
