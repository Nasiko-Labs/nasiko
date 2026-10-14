import {
  Button,
  Card,
  CardContent,
  Skeleton,
  Switch,
  Tabs,
  TabsList,
  TabsTrigger,
  TabsContent,
} from "@nasiko/ui/primitives";
import { CopyButton } from "@nasiko/ui/composites";
import { Reveal, StaggerGroup } from "@nasiko/ui/motion";

export default function RecipesFixture() {
  return (
    <div className="flex flex-col gap-3">
      <section aria-label="Operational feedback fixtures" className="flex flex-col gap-3">
        <Button data-testid="feedback-button">Run fixture</Button>
        <Switch aria-label="Fixture switch" />
        <CopyButton text="https://example.test/local-fixture" label="Copy fixture endpoint" />
        <Skeleton data-testid="feedback-skeleton" className="h-8 w-40" />
        <Tabs defaultValue="metrics">
          <TabsList>
            <TabsTrigger value="metrics">Fixture metrics</TabsTrigger>
            <TabsTrigger value="logs">Fixture logs</TabsTrigger>
          </TabsList>
          <TabsContent value="metrics">Metrics fixture</TabsContent>
          <TabsContent value="logs">Logs fixture</TabsContent>
        </Tabs>
      </section>
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
