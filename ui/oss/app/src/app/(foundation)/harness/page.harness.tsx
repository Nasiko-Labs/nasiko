import Link from "@nasiko/app-core/navigation";

/**
 * Private verification harness index. Only exists in builds where
 * Vite admits the fixture route module (NASIKO_HARNESS=1 in development or an explicit test build).
 */
export default function HarnessIndex() {
  return (
    <section aria-labelledby="h-title" className="flex flex-col gap-3">
      <h1 id="h-title" className="text-xl font-semibold">
        Verification harness
      </h1>
      <p className="text-muted-foreground">Synthetic fixtures only. Not a product screen.</p>
      <ul className="list-disc pl-5">
        <li>
          <Link href="/harness/perf" className="underline">
            Performance marks smoke
          </Link>
        </li>
      </ul>
    </section>
  );
}
