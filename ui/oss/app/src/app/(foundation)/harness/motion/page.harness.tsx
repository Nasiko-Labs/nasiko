import Link from "@nasiko/app-core/navigation";

export default function MotionHarnessIndex() {
  return (
    <ul className="list-disc pl-5 text-sm">
      {["recipes", "panel", "kpi", "nav/usage"].map((p) => (
        <li key={p}>
          <Link href={`/harness/motion/${p}`} className="underline">
            {p}
          </Link>
        </li>
      ))}
    </ul>
  );
}
