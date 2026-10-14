import type { Meta, StoryObj } from "@storybook/react-vite";
import { Alert, AlertDescription, AlertTitle } from "./alert";
import { Badge } from "./badge";
import { Button } from "./button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "./card";
import { Checkbox } from "./checkbox";
import { Input } from "./input";
import { Label } from "./label";
import { Progress } from "./progress";
import { RadioGroup, RadioGroupItem } from "./radio-group";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "./select";
import { Skeleton } from "./skeleton";
import { Switch } from "./switch";
import { Tooltip, TooltipContent, TooltipTrigger } from "./tooltip";

/** shadcn base-nova on Base UI — the one primitive family (vendored; see REGISTRY.json). */
const meta = { title: "Primitives (shadcn base-nova)/Overview" } satisfies Meta;
export default meta;

export const Overview: StoryObj = {
  render: () => (
    <div className="grid gap-6 md:grid-cols-2">
      <Card>
        <CardHeader>
          <CardTitle>Controls</CardTitle>
          <CardDescription>Button variants, inputs, selection</CardDescription>
        </CardHeader>
        <CardContent className="flex flex-col gap-3">
          <div className="flex flex-wrap gap-2">
            <Button>Default</Button>
            <Button variant="outline">Outline</Button>
            <Button variant="secondary">Secondary</Button>
            <Button variant="ghost">Ghost</Button>
            <Button variant="destructive">Destructive</Button>
            <Button disabled>Disabled</Button>
          </div>
          <div className="flex flex-col gap-1">
            <Label htmlFor="in">Agent name</Label>
            <Input id="in" placeholder="alpha-agent" />
          </div>
          <Select
            defaultValue="gpt"
            items={[
              { value: "gpt", label: "gpt-x" },
              { value: "nova", label: "Nova" },
            ]}
          >
            <SelectTrigger aria-label="Model">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="gpt">gpt-x</SelectItem>
              <SelectItem value="nova">Nova</SelectItem>
            </SelectContent>
          </Select>
          <RadioGroup aria-label="Tier" defaultValue="medium" className="flex gap-4">
            <Label className="flex items-center gap-2">
              <RadioGroupItem value="low" /> Low
            </Label>
            <Label className="flex items-center gap-2">
              <RadioGroupItem value="medium" /> Medium
            </Label>
          </RadioGroup>
          <div className="flex items-center gap-3">
            <Label className="flex items-center gap-2">
              <Checkbox defaultChecked /> Checkbox
            </Label>
            <Label className="flex items-center gap-2">
              <Switch defaultChecked /> Switch
            </Label>
          </div>
        </CardContent>
      </Card>
      <Card>
        <CardHeader>
          <CardTitle>Feedback</CardTitle>
          <CardDescription>Badges, alert, progress, skeleton, tooltip</CardDescription>
        </CardHeader>
        <CardContent className="flex flex-col gap-3">
          <div className="flex flex-wrap gap-2">
            <Badge>Default</Badge>
            <Badge variant="secondary">Secondary</Badge>
            <Badge variant="outline">Outline</Badge>
            <Badge variant="destructive">Destructive</Badge>
          </div>
          <Alert>
            <AlertTitle>Heads up</AlertTitle>
            <AlertDescription>Alerts are immediate; nothing animates them in.</AlertDescription>
          </Alert>
          <Progress value={62} aria-label="Budget used" />
          <Skeleton className="h-4 w-2/3" />
          <Tooltip>
            <TooltipTrigger render={<Button variant="outline" size="sm" />}>
              Hover or focus me
            </TooltipTrigger>
            <TooltipContent>Tooltip content</TooltipContent>
          </Tooltip>
        </CardContent>
      </Card>
    </div>
  ),
};
