'use client'

import { motion, useReducedMotion, type Variants } from 'motion/react'
import { type ComponentProps, useCallback } from 'react'
import { cn } from '@/lib/utils'

export type ShimmeringTextProps = Omit<ComponentProps<typeof motion.span>, 'children'> & {
  /** The text to render with the shimmering effect. */
  text: string
  /**
   * Duration in seconds for one shimmer cycle.
   * @defaultValue 1
   */
  duration?: number
  /**
   * Pause the shimmer (e.g. when the hero leaves the viewport).
   * @defaultValue false
   */
  paused?: boolean
  /**
   * Legacy alias for `paused`.
   * @defaultValue false
   */
  isStopped?: boolean
}

export function ShimmeringText({
  text,
  duration = 1,
  isStopped = false,
  paused = false,
  className,
  ...props
}: ShimmeringTextProps) {
  const reducedMotion = useReducedMotion()
  const stopped = isStopped || paused || reducedMotion === true

  const createCharVariants = useCallback(
    (charIndex: number): Variants => ({
      running: {
        color: ['var(--color)', 'var(--shimmering-color)', 'var(--color)'],
        transition: {
          duration,
          repeat: Number.POSITIVE_INFINITY,
          repeatType: 'loop',
          repeatDelay: text.length * 0.05,
          delay: (charIndex * duration) / text.length,
          ease: 'easeInOut',
        },
      },
      stopped: {
        color: 'var(--color)',
        transition: {
          duration: duration * 0.5,
          ease: 'easeOut',
        },
      },
    }),
    [duration, text.length],
  )

  return (
    <motion.span
      className={cn(
        'inline-flex items-center leading-none select-none',
        '[--color:var(--muted-foreground)] [--shimmering-color:var(--foreground)]',
        className,
      )}
      {...props}
    >
      {text.split('').map((char, index) => (
        <motion.span
          animate={stopped ? 'stopped' : 'running'}
          aria-hidden
          className="inline-block leading-none whitespace-pre"
          initial="stopped"
          // eslint-disable-next-line @eslint-react/no-array-index-key -- one span per character of a static label; position is its identity
          key={index}
          variants={createCharVariants(index)}
        >
          {char}
        </motion.span>
      ))}
      <span className="sr-only">{text}</span>
    </motion.span>
  )
}
