import { useParams } from "@nasiko/app-core/navigation";
import { Card, CardContent, CardHeader, CardTitle } from "@nasiko/ui/primitives";
import { Reveal } from "@nasiko/ui/motion";

export default function SectionPage() {
  const { section } = useParams<{ section: string }>();
  if (section === "broken") throw new Error("synthetic route error");
  return (
    <Reveal recipe="section" data-testid={`section-${section}`}>
      <Card size="sm">
        <CardHeader>
          <CardTitle>{section}</CardTitle>
        </CardHeader>
        <CardContent>Only this boundary changed; the parent shell did not remount.</CardContent>
      </Card>
    </Reveal>
  );
}
