// The jest-dom matchers and Testing Library cleanup for the web test project. The matchers are registered on this
// package's own `vitest` (the runner's instance): `@testing-library/jest-dom/vitest` would extend whichever vitest
// copy pnpm linked for jest-dom's peers, and a second copy breaks `rejects.toThrow(<string>)` (seen when the
// workspace app moved into packages, ADR-0007).
import * as matchers from "@testing-library/jest-dom/matchers";
import { cleanup } from "@testing-library/react";
import { afterEach, expect } from "vitest";

expect.extend(matchers);

afterEach(() => {
  cleanup();
});
