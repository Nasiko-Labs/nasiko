// Ported from the legacy ui/ on development (3aac84f66) common/src/features/home/useOnScreen.ts. Local edits: Prettier formatting.
import { useEffect, useState } from "react";
import { ROW_DWELL_MS, ROW_MARGIN } from "./tuning";

/**
 * Whether the element is on screen: true once it has stayed within `ROW_MARGIN` of the viewport for `ROW_DWELL_MS`
 * (rows flung past start nothing), false again when it leaves. Without IntersectionObserver (old engines) every row
 * counts as on screen, so the cells still fill in.
 */
// eslint-disable-next-line @typescript-eslint/no-unnecessary-type-parameters -- the caller names its element type, so its ref callback fits
export function useOnScreen<E extends Element>(): [(el: E | null) => void, boolean] {
  const [el, setEl] = useState<E | null>(null);
  const [on, setOn] = useState(typeof IntersectionObserver === "undefined");
  useEffect(() => {
    if (!el || typeof IntersectionObserver === "undefined") return;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const io = new IntersectionObserver(
      (entries) => {
        // The newest entry wins: a busy frame can deliver an enter and a leave together.
        const inside = entries.at(-1)?.isIntersecting ?? false;
        clearTimeout(timer);
        if (inside)
          timer = setTimeout(() => {
            setOn(true);
          }, ROW_DWELL_MS);
        else setOn(false);
      },
      { rootMargin: ROW_MARGIN },
    );
    io.observe(el);
    return () => {
      clearTimeout(timer);
      io.disconnect();
    };
  }, [el]);
  return [setEl, on];
}
