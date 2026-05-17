import { LogsView } from "@/components/LogsView";

export const metadata = {
  title: "Platform Logs | Nasiko",
  description:
    "Activity log for Nasiko — recent platform events synthesized from observability data.",
};

export default function LogsPage() {
  return <LogsView />;
}
