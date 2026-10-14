import type { Meta, StoryObj } from "@storybook/react-vite";
import { PerfMarksSmoke } from "./perf-marks-smoke";

const meta = { title: "Harness/PerfMarksSmoke", component: PerfMarksSmoke } satisfies Meta<
  typeof PerfMarksSmoke
>;
export default meta;
export const Default: StoryObj<typeof meta> = {};
