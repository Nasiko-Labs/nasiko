// Saved character design. Animation never writes into this object: the
// transient animated pose lives in anim/channels.ts and is layered on top.

import { ART_COLORS } from '../character/art'
import { CACTUS_COLORS } from '../character/cactusArt'

export const SCHEMA_ID = 'weave-monster-character'
export const SCHEMA_VERSION = 1

export type EngineId = 'motion' | 'gsap' | 'anime'
/** which character the design shows */
export type CharacterId = 'monster' | 'cactus'
export const CHARACTERS: { id: CharacterId; label: string }[] = [
  { id: 'monster', label: 'Monster' },
  { id: 'cactus', label: 'Cactus' },
]
/** held = tools sit in the near hand; float = they hover beside the body with a gap */
export type PropPlacement = 'held' | 'float'
export type PropId = 'cap' | 'blueprint' | 'wrench' | 'glass' | 'tiles'
export type EffectId = 'marks' | 'sweat' | 'question' | 'bulb' | 'sparkle'
/** eyes = pupils only; head = pupils + the head turns; body = pupils, head, then the body follows */
export type TrackingMode = 'off' | 'eyes' | 'head' | 'body'
export type EasingId = 'standard' | 'gentle' | 'snappy' | 'linear' | 'custom'
export type Bezier = [number, number, number, number]

/** Expressions: the core set, then extra ones in the same drawing language. */
export const CORE_EXPRESSIONS = [
  'neutral',
  'attentive',
  'curious',
  'surprised',
  'focused',
  'engaged',
  'thinking',
  'delighted',
  'concerned',
  'resting',
  'relaxed',
] as const
export const EXTRA_EXPRESSIONS = [
  'excited',
  'shy',
  'content',
  'determined',
  'laughing',
  'wink',
  'sleepy',
  'smug',
  'worried',
  'sad',
  'shocked',
  'sideEye',
  'bored',
  'grumpy',
] as const
export const EXPRESSIONS = [...CORE_EXPRESSIONS, ...EXTRA_EXPRESSIONS] as const
export type ExpressionId = (typeof EXPRESSIONS)[number]

export const STATES = [
  'idle',
  'attentive',
  'curious',
  'thinking',
  'building',
  'fixing',
  'inspecting',
  'arranging',
  'success',
  'concerned',
  'waving',
] as const
export type StateId = (typeof STATES)[number]

export const PROP_IDS: PropId[] = ['cap', 'blueprint', 'wrench', 'glass', 'tiles']
export const EFFECT_IDS: EffectId[] = ['marks', 'sweat', 'question', 'bulb', 'sparkle']

export interface PropConfig {
  enabled: boolean
  size: number // multiplier
  rotation: number // deg
  offsetX: number // body half-width units
  offsetY: number // body half-height units
}

export interface SequenceStep {
  id: string
  kind: 'state' | 'expression' | 'clip'
  ref: string
  hold: number // ms
}

export interface Sequence {
  id: string
  name: string
  steps: SequenceStep[]
}

export interface ClipTuning {
  durationScale: number
  amplitude: number
  easing: EasingId | 'authored'
  customBezier: Bezier
}

export interface Colors {
  body: string
  dark: string // far arm, creases, left spots
  spotLight: string // right spots
  face: string // face patch
  toe: string
  leaf: string
  leafLight: string
  eyeWhite: string
  ink: string // pupils, mouth
  mouthInside: string
  tongue: string
  blush: string
  effects: string // action marks, Zs, question mark
  accessory: string // blueprint, tiles, magnifier, cap
  accessoryMid: string
  accessoryLight: string
  accessoryDeep: string
  metal: string // wrench
  // the cactus (reference/cactus-rest.png)
  cactusBody: string
  cactusShade: string // the darker band down the column's right side
  cactusArm: string
  spine: string
  potRim: string
  pot: string
  stage: string
}

/** The puppet's look and its 2.5D behaviour. */
export interface MonsterBody {
  preset: 'monster' | 'custom'
  palette: string // id of the colour palette last applied ('custom' after edits)
  width: number // multiplier
  height: number // multiplier
  leaves: boolean
  leafSize: number
  ears: boolean
  earSize: number
  spots: boolean
  creases: boolean
  toes: boolean
  armSize: number
  /** deg the surface turns at turn = ±1 (features wrap round the body) */
  turnRange: number
  /** deg the face tips up/down at tip = ±1 */
  tipRange: number
  /** 0..2 how far ears, leaves and arms shift with depth when turning */
  depth: number
  /** 0..1 how much the outline bulges toward the turn */
  silhouette: number
  /** resting turn, −0.5..0.5 (0 = exactly the art: a slight ¾ to the left) */
  restTurn: number
  /** floor contact shadow 0..1 */
  shadow: number
  /** 0..1 how much darker the arm is while it hangs behind the body (the art: 1) */
  armShade: number
}

export interface CharacterConfig {
  schema: typeof SCHEMA_ID
  version: number
  name: string
  character: CharacterId
  colors: Colors
  body: MonsterBody
  eyes: {
    pupilSize: number // multiplier
    intensity: number // expression intensity 0..2
    mobility: number // how far gaze carries the pupils, 0..1
    edgePadding: number // true distance between a pupil and the eye mask's edge, px
    mouthWidth: number // line width multiplier
    blush: boolean // allow blush
    teeth: boolean // the two little fangs on the smile
  }
  props: Record<PropId, PropConfig> & {
    entranceMs: number
    exitMs: number
    motionAmp: number
    placement: PropPlacement
  }
  effects: Record<EffectId, boolean> & { size: number }
  interaction: {
    tracking: TrackingMode
    gazeStrength: number // 0..1
    trackingRadius: number // px
    smoothing: number // 0..1
    headTurn: number // 0..1 how far the head turns toward the pointer (of the turn range)
    bodyFollow: number // 0..1 how much the body follows the head
    hover: boolean
    hoverLean: number // deg the body leans toward the pointer on hover
    clickAck: boolean
  }
  motion: {
    breathing: boolean
    breathPeriod: number // ms
    breathAmount: number // % belly expansion
    morphAmount: number // local deformation multiplier (bulge, bend)
    drift: number // ambient part drift amount, 0..2
    driftPeriod: number // ms for one full drift cycle
    jelly: number // secondary follow-through (arms, ears, leaves, body), 0..2
    squash: number // 0..1
    bounce: number // 0..1 overshoot / hop height
    settle: number // 0..1 settling wobble
    blinks: boolean
    blinkInterval: number // ms average
    blinkJitter: number // 0..1
    blinkDuration: number // ms
    gestureIntensity: number
    transitionMs: number
    easing: EasingId
    customBezier: Bezier
  }
  clip: ClipTuning
  engine: EngineId
  sequences: Sequence[]
}

/** Exact colours from reference/monster-rest.png; the extras are designed to sit with them. */
export const PALETTE: Colors = {
  body: ART_COLORS.body,
  dark: ART_COLORS.dark,
  spotLight: ART_COLORS.spotLight,
  face: ART_COLORS.face,
  toe: ART_COLORS.toe,
  leaf: ART_COLORS.leaf,
  leafLight: ART_COLORS.leafLight,
  eyeWhite: ART_COLORS.eyeWhite,
  ink: ART_COLORS.ink,
  mouthInside: '#3A1206',
  tongue: '#F26D5B',
  blush: '#FF8A7A',
  effects: '#F3AC3D',
  // tools: the Figma lab's blues and wrench grey
  accessory: '#3C5FD0',
  accessoryMid: '#6383EB',
  accessoryLight: '#9CB1F3',
  accessoryDeep: '#2C4CB1',
  metal: '#A2A2A2',
  cactusBody: CACTUS_COLORS.body,
  cactusShade: CACTUS_COLORS.shade,
  cactusArm: CACTUS_COLORS.arm,
  spine: CACTUS_COLORS.spine,
  potRim: CACTUS_COLORS.potRim,
  pot: CACTUS_COLORS.pot,
  stage: ART_COLORS.stage,
}

const prop = (o: Partial<PropConfig> = {}): PropConfig => ({
  enabled: true,
  size: 1,
  rotation: 0,
  offsetX: 0,
  offsetY: 0,
  ...o,
})

export function monsterBody(): MonsterBody {
  return {
    preset: 'monster',
    palette: 'orange',
    width: 1,
    height: 1,
    leaves: true,
    leafSize: 1,
    ears: true,
    earSize: 1,
    spots: true,
    creases: true,
    toes: true,
    armSize: 1,
    turnRange: 62,
    tipRange: 24,
    depth: 1,
    silhouette: 0.5,
    restTurn: 0,
    shadow: 0.35,
    // the arms read as one smooth shape with the body (user, round 3)
    armShade: 0,
  }
}

export function defaultConfig(): CharacterConfig {
  return {
    schema: SCHEMA_ID,
    version: SCHEMA_VERSION,
    name: 'Weave',
    character: 'monster',
    colors: { ...PALETTE },
    body: monsterBody(),
    eyes: {
      pupilSize: 1,
      intensity: 1,
      mobility: 0.85,
      edgePadding: 10,
      mouthWidth: 1,
      blush: true,
      teeth: true,
    },
    props: {
      // no engineer's cap unless asked (standing preference)
      cap: prop({ enabled: false }),
      // not needed for this character (user, round 2); the toggle stays
      blueprint: prop({ enabled: false }),
      wrench: prop(),
      glass: prop(),
      tiles: prop(),
      entranceMs: 420,
      exitMs: 260,
      motionAmp: 1,
      placement: 'held',
    },
    effects: { marks: true, sweat: true, question: true, bulb: true, sparkle: false, size: 1 },
    interaction: {
      tracking: 'body',
      gazeStrength: 1,
      trackingRadius: 700,
      smoothing: 0.55,
      headTurn: 0.38,
      bodyFollow: 0.45,
      hover: true,
      hoverLean: 4,
      clickAck: true,
    },
    motion: {
      breathing: true,
      breathPeriod: 3800,
      breathAmount: 2.2,
      // bouncy and soft, but never squiggly
      morphAmount: 0.8,
      drift: 0.8,
      driftPeriod: 14000,
      jelly: 0.9,
      squash: 0.75,
      bounce: 0.7,
      settle: 0.6,
      blinks: true,
      blinkInterval: 3800,
      blinkJitter: 0.55,
      blinkDuration: 170,
      gestureIntensity: 1,
      transitionMs: 460,
      easing: 'standard',
      customBezier: [0.4, 0, 0.2, 1],
    },
    clip: { durationScale: 1, amplitude: 1, easing: 'authored', customBezier: [0.4, 0, 0.2, 1] },
    engine: 'gsap',
    sequences: [
      {
        id: 'seq-demo',
        name: 'Notice, wave, build, cheer',
        steps: [
          { id: 's1', kind: 'state', ref: 'attentive', hold: 1100 },
          { id: 's2', kind: 'state', ref: 'waving', hold: 2200 },
          { id: 's3', kind: 'state', ref: 'thinking', hold: 1600 },
          { id: 's4', kind: 'state', ref: 'building', hold: 4200 },
          { id: 's5', kind: 'state', ref: 'success', hold: 1700 },
          { id: 's6', kind: 'state', ref: 'idle', hold: 600 },
        ],
      },
    ],
  }
}

// Safe numeric ranges used by the inspector, validation and clamping.
export interface Range {
  min: number
  max: number
  step: number
}
const r = (min: number, max: number, step: number): Range => ({ min, max, step })

export const RANGES = {
  body: {
    width: r(0.75, 1.3, 0.01),
    height: r(0.75, 1.3, 0.01),
    leafSize: r(0.5, 1.6, 0.01),
    earSize: r(0.4, 1.8, 0.01),
    armSize: r(0.7, 1.3, 0.01),
    turnRange: r(0, 85, 0.5),
    tipRange: r(0, 40, 0.5),
    depth: r(0, 2, 0.01),
    silhouette: r(0, 1, 0.01),
    restTurn: r(-0.5, 0.5, 0.01),
    shadow: r(0, 1, 0.01),
    armShade: r(0, 1, 0.01),
  },
  eyes: {
    pupilSize: r(0.6, 1.5, 0.01),
    intensity: r(0, 2, 0.01),
    mobility: r(0, 1, 0.01),
    edgePadding: r(2, 30, 1),
    mouthWidth: r(0.5, 2, 0.01),
  },
  prop: {
    size: r(0.5, 1.8, 0.01),
    rotation: r(-45, 45, 1),
    offsetX: r(-0.6, 0.6, 0.01),
    offsetY: r(-0.6, 0.6, 0.01),
  },
  propsShared: { entranceMs: r(80, 1200, 10), exitMs: r(60, 1000, 10), motionAmp: r(0, 2, 0.01) },
  effects: { size: r(0.5, 1.8, 0.01) },
  interaction: {
    gazeStrength: r(0, 1, 0.01),
    trackingRadius: r(80, 1200, 10),
    smoothing: r(0, 0.95, 0.01),
    headTurn: r(0, 1, 0.01),
    bodyFollow: r(0, 1, 0.01),
    hoverLean: r(0, 12, 0.5),
  },
  motion: {
    breathPeriod: r(2000, 8000, 50),
    breathAmount: r(0, 6, 0.1),
    morphAmount: r(0, 2, 0.01),
    drift: r(0, 2, 0.01),
    driftPeriod: r(6000, 30000, 250),
    jelly: r(0, 2, 0.01),
    squash: r(0, 1, 0.01),
    bounce: r(0, 1, 0.01),
    settle: r(0, 1, 0.01),
    blinkInterval: r(1200, 12000, 100),
    blinkJitter: r(0, 1, 0.01),
    blinkDuration: r(80, 400, 5),
    gestureIntensity: r(0, 2, 0.01),
    transitionMs: r(0, 1500, 10),
  },
  clip: { durationScale: r(0.25, 3, 0.05), amplitude: r(0, 2, 0.01) },
  sequenceHold: r(100, 10000, 50),
} as const

export const clamp = (v: number, min: number, max: number) => (v < min ? min : v > max ? max : v)
export const clampR = (v: number, range: Range) => clamp(v, range.min, range.max)
