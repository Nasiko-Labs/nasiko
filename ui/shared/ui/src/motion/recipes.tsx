"use client";

import { AnimatePresence } from "motion/react";
import * as m from "motion/react-m";
import {
  Children,
  useEffect,
  useId,
  useRef,
  useState,
  type ComponentPropsWithoutRef,
  type ReactNode,
} from "react";
import { cn } from "../lib/cn";
import { EASING, staggerDelayMs, type EffectiveRecipe, type MotionRecipe } from "./policy";
import { useHydrated, useMotion } from "./provider";

/**
 * Recipe adapters (contract §8.6). Each takes a semantic recipe name, asks the
 * host what it may do, and renders accordingly. None of them can be handed a
 * duration, easing or keyframe; none animates layout.
 */

function transitionFor(r: EffectiveRecipe) {
  return { duration: r.durationMs / 1000, ease: EASING.out };
}

type MDivProps = ComponentPropsWithoutRef<typeof m.div>;

export interface RevealProps extends Omit<
  MDivProps,
  "initial" | "animate" | "exit" | "transition"
> {
  readonly recipe?: Extract<MotionRecipe, "reveal" | "section" | "panel" | "showcase">;
  /** Position in a stagger group; the host caps the delay. */
  readonly index?: number;
  /** Where the element travels from at `full`. */
  readonly from?: "below" | "above" | "start" | "end";
  readonly children?: ReactNode;
}

/**
 * One-time entrance for content that just became available (a subtree that
 * became valid, a card that arrived). Plays once per mount and never on a
 * restored surface. In reduced mode it is an opacity change only; in none it
 * renders in place.
 */
export function Reveal({
  recipe = "reveal",
  index = 0,
  from = "below",
  children,
  className,
  ...rest
}: RevealProps) {
  const host = useMotion();
  const hydrated = useHydrated();
  const r = host.recipe(recipe);
  // Server-rendered content is final: no entrance state in the HTML, no replay
  // after hydration. Only elements mounted on the client animate in.
  const still = !hydrated || r.durationMs === 0 || !host.enter;
  const trace = host.trace;
  const id = useId();
  useEffect(() => {
    // Recorded whenever the element is (or becomes) still — including the
    // post-hydration re-resolution when the OS preference only becomes known on
    // the client. Diagnostic only; nothing renders differently because of it.
    if (still)
      trace?.({
        id,
        adapter: "reveal",
        phase: "skipped",
        recipe,
        effective: r,
        at: performance.now(),
      });
    // eslint-disable-next-line react-hooks/exhaustive-deps -- `r` derives from the same inputs as `still`
  }, [still, id, recipe, trace]);
  if (still) {
    return (
      <m.div className={className} {...rest}>
        {children}
      </m.div>
    );
  }
  const d = r.distancePx;
  const offset = r.opacityOnly
    ? {}
    : from === "below"
      ? { y: d }
      : from === "above"
        ? { y: -d }
        : from === "start"
          ? { x: -d }
          : { x: d };
  return (
    <m.div
      className={className}
      initial={{ opacity: 0, ...offset }}
      animate={{ opacity: 1, x: 0, y: 0 }}
      transition={{ ...transitionFor(r), delay: staggerDelayMs(index, host.mode) / 1000 }}
      onAnimationStart={() =>
        trace?.({
          id,
          adapter: "reveal",
          phase: "start",
          recipe,
          effective: r,
          at: performance.now(),
        })
      }
      onAnimationComplete={() =>
        trace?.({
          id,
          adapter: "reveal",
          phase: "complete",
          recipe,
          effective: r,
          at: performance.now(),
        })
      }
      {...rest}
    >
      {children}
    </m.div>
  );
}

/**
 * Bounded stagger for a small set of siblings (≤ 6 get a delay; the rest arrive
 * with the sixth). Never for rows, tokens or streamed chunks.
 */
export function StaggerGroup({
  children,
  recipe = "reveal",
  className,
}: {
  children: ReactNode;
  recipe?: RevealProps["recipe"];
  className?: string;
}) {
  return (
    <div className={className}>
      {Children.map(children, (child, i) => (
        <Reveal recipe={recipe} index={i}>
          {child}
        </Reveal>
      ))}
    </div>
  );
}

export interface PresenceProps {
  readonly show: boolean;
  readonly recipe?: Extract<MotionRecipe, "panel" | "reveal" | "section">;
  readonly from?: RevealProps["from"];
  readonly className?: string;
  readonly children: ReactNode;
  /** Rendered immediately on exit so a departing panel cannot take stale input. */
  readonly inertOnExit?: boolean;
}

/**
 * Enter/exit for a panel, drawer region or expandable section. The exit is
 * interruptible: toggling `show` back during the exit re-enters from the
 * current value. Logical state (focus restore, dismissal, cancellation) is the
 * caller's and never waits for the exit to finish.
 */
export function Presence({
  show,
  recipe = "panel",
  from = "below",
  className,
  children,
  inertOnExit = true,
}: PresenceProps) {
  const host = useMotion();
  const hydrated = useHydrated();
  const r = host.recipe(recipe);
  const id = useId();
  const ref = useRef<HTMLDivElement | null>(null);
  // AnimatePresence keeps the departing element rendered with its last props,
  // so inertness is applied imperatively the moment `show` turns false: a
  // panel on its way out can never take a click or keep focus.
  useEffect(() => {
    if (!show && inertOnExit && ref.current) ref.current.inert = true;
  }, [show, inertOnExit]);
  if (!hydrated || r.durationMs === 0) {
    return show ? <div className={className}>{children}</div> : null;
  }
  const d = r.distancePx;
  const offset = r.opacityOnly
    ? {}
    : from === "below"
      ? { y: d }
      : from === "above"
        ? { y: -d }
        : from === "start"
          ? { x: -d }
          : { x: d };
  return (
    <AnimatePresence initial={host.enter}>
      {show ? (
        <m.div
          key="presence"
          ref={ref}
          className={className}
          initial={{ opacity: 0, ...offset }}
          animate={{ opacity: 1, x: 0, y: 0 }}
          exit={{
            opacity: 0,
            ...offset,
            transition: { duration: Math.min(r.durationMs, 120) / 1000, ease: EASING.out },
          }}
          transition={transitionFor(r)}
          onAnimationStart={() =>
            host.trace?.({
              id,
              adapter: "presence",
              phase: "start",
              recipe,
              effective: r,
              at: performance.now(),
            })
          }
          onAnimationComplete={() =>
            host.trace?.({
              id,
              adapter: "presence",
              phase: "complete",
              recipe,
              effective: r,
              at: performance.now(),
            })
          }
        >
          {children}
        </m.div>
      ) : null}
    </AnimatePresence>
  );
}

/**
 * Brief changed-state emphasis for a real value change (a KPI after refetch,
 * a timeline event that actually arrived). Colour only, finite, and the value
 * itself is rendered immediately — there is no count-up and no intermediate
 * number (§8.3 KPI / financial values).
 */
export function Emphasis({
  changeKey,
  className,
  children,
}: {
  changeKey: string | number;
  className?: string;
  children: ReactNode;
}) {
  const host = useMotion();
  const r = host.recipe("emphasis");
  // "Adjust state while rendering": a new key turns the highlight on in the same
  // render, so the value and its emphasis paint together; the effect only turns
  // it off again. First render never highlights (nothing changed yet).
  const [seenKey, setSeenKey] = useState(changeKey);
  const [active, setActive] = useState(false);
  if (changeKey !== seenKey) {
    setSeenKey(changeKey);
    if (r.durationMs > 0) setActive(true);
  }
  useEffect(() => {
    if (!active) return undefined;
    const t = setTimeout(() => {
      setActive(false);
    }, r.durationMs * 4);
    return () => {
      clearTimeout(t);
    };
  }, [active, seenKey, r.durationMs]);
  return (
    <span
      data-emphasis={active ? "on" : "off"}
      className={cn(
        "rounded-sm transition-colors duration-micro ease-standard data-[emphasis=on]:bg-info/15",
        className,
      )}
    >
      {children}
    </span>
  );
}
