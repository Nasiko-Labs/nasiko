import type { Meta, StoryObj } from "@storybook/react-vite";
import { BotIcon, CoinsIcon, GaugeIcon, HomeIcon, RouteIcon, SettingsIcon } from "lucide-react";
import { userEvent, within } from "storybook/test";
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarGroup,
  SidebarGroupContent,
  SidebarGroupLabel,
  SidebarHeader,
  SidebarInset,
  SidebarMenu,
  SidebarMenuBadge,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarProvider,
  SidebarRail,
  SidebarSeparator,
  SidebarTrigger,
} from "./sidebar";

/**
 * The lab's shadcn sidebar (shared/ui/src/primitives/sidebar.tsx): 240 px expanded, a 56 px icon
 * rail collapsed (tooltips only there), and a sheet below `md`. ⌘B / Ctrl+B toggles it. Synthetic nav.
 */
const meta = { title: "Primitives (shadcn base-nova)/Sidebar" } satisfies Meta;
export default meta;

const GROUPS = [
  {
    label: "Fleet",
    items: [
      { label: "Overview", icon: HomeIcon, active: true },
      { label: "Agents", icon: BotIcon, badge: "3" },
    ],
  },
  {
    label: "Observe",
    items: [
      { label: "TokenOps", icon: CoinsIcon },
      { label: "Optimization", icon: GaugeIcon },
      { label: "LLM router", icon: RouteIcon },
    ],
  },
] as const;

function Shell({ defaultOpen }: { defaultOpen: boolean }) {
  return (
    <SidebarProvider defaultOpen={defaultOpen} className="min-h-[32rem]">
      <Sidebar collapsible="icon">
        <SidebarHeader>
          <SidebarMenu>
            <SidebarMenuItem>
              <SidebarMenuButton size="lg" tooltip="OpenRuntime">
                <span className="flex size-8 shrink-0 items-center justify-center rounded-md bg-sidebar-primary text-sidebar-primary-foreground">
                  N
                </span>
                <span className="font-semibold">OpenRuntime</span>
              </SidebarMenuButton>
            </SidebarMenuItem>
          </SidebarMenu>
        </SidebarHeader>
        <SidebarSeparator />
        <SidebarContent>
          {GROUPS.map((g) => (
            <SidebarGroup key={g.label}>
              <SidebarGroupLabel>{g.label}</SidebarGroupLabel>
              <SidebarGroupContent>
                <SidebarMenu>
                  {g.items.map((item) => (
                    <SidebarMenuItem key={item.label}>
                      <SidebarMenuButton
                        tooltip={item.label}
                        isActive={"active" in item && item.active}
                        render={<a href="#" />}
                      >
                        <item.icon aria-hidden />
                        <span>{item.label}</span>
                      </SidebarMenuButton>
                      {"badge" in item ? <SidebarMenuBadge>{item.badge}</SidebarMenuBadge> : null}
                    </SidebarMenuItem>
                  ))}
                </SidebarMenu>
              </SidebarGroupContent>
            </SidebarGroup>
          ))}
        </SidebarContent>
        <SidebarFooter>
          <SidebarMenu>
            <SidebarMenuItem>
              <SidebarMenuButton tooltip="Settings">
                <SettingsIcon aria-hidden />
                <span>Settings</span>
              </SidebarMenuButton>
            </SidebarMenuItem>
          </SidebarMenu>
        </SidebarFooter>
        <SidebarRail />
      </Sidebar>
      <SidebarInset>
        <header className="flex h-12 items-center gap-2 border-b px-4">
          <SidebarTrigger />
          <h1 className="text-sm font-semibold">Overview</h1>
        </header>
        <div className="p-4 text-sm text-muted-foreground">
          The page. Press ⌘B or Ctrl+B, or the trigger, to collapse the sidebar; below 768 px it
          opens as a sheet.
        </div>
      </SidebarInset>
    </SidebarProvider>
  );
}

export const Expanded: StoryObj = { render: () => <Shell defaultOpen /> };

export const CollapsedRail: StoryObj = { render: () => <Shell defaultOpen={false} /> };

/** On a phone-width viewport the trigger opens the sheet; on desktop it toggles the rail. */
export const PhoneSheet: StoryObj = {
  render: () => <Shell defaultOpen />,
  play: async ({ canvasElement }) => {
    const c = within(canvasElement);
    await userEvent.click(c.getByRole("button", { name: "Toggle Sidebar" }));
  },
};
