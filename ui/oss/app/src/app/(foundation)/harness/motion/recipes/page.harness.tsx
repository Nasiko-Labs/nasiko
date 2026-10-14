import { Card, CardContent } from "@nasiko/ui/primitives";
import { Reveal, StaggerGroup } from "@nasiko/ui/motion";

export default function RecipesFixture() {
  return (
    <div className="flex flex-col gap-3">
      {(["reveal", "section", "panel", "showcase"] as const).map((r) => (
        <Reveal key={r} recipe={r} data-testid={`reveal-${r}`}>
          <Card size="sm">
            <CardContent>{r}</CardContent>
          </Card>
        </Reveal>
      ))}
      <StaggerGroup className="grid grid-cols-5 gap-2">
        {Array.from({ length: 10 }, (_, i) => (
          <div
            key={i}
            data-testid="stagger-item"
            className="rounded bg-muted p-2 text-center text-xs"
          >
            {i + 1}
          </div>
        ))}
      </StaggerGroup>
    </div>
  );
}
