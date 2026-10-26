import { cn } from "@/lib/report";

export function Marquee({
  items,
  className,
}: {
  items: string[];
  className?: string;
}) {
  const loop = [...items, ...items];
  return (
    <div className={cn("relative overflow-hidden border-y border-rule bg-forest text-cream", className)}>
      <div className="flex w-max animate-marquee motion-reduce:animate-none">
        {loop.map((item, i) => (
          <span
            key={`${item}-${i}`}
            className="flex items-center gap-6 px-8 py-3 font-mono text-[11px] uppercase tracking-[0.28em]"
          >
            <i className="inline-block h-1.5 w-1.5 rounded-full bg-phosphor" aria-hidden />
            {item}
          </span>
        ))}
      </div>
    </div>
  );
}
