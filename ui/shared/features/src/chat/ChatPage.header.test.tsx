// Ported from nasiko-ui-lab@1b99bdda common/src/features/chat/ChatPage.header.test.tsx. Local edits:
// `setupMockServer()` starts the msw server for the file (the lab started it globally in its test setup);
// `@nasiko/ui/lib` is mocked partially (only `downloadText`: the lab mocked its one-function `@/lib/download`); the
// View session test stops at the URL (the trace page is not ported yet); Prettier formatting.
/**
 * The chat header's actions: Export as Markdown, rename (focus, a refused save, an early blur) and
 * View session.
 */
import { fireEvent, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { afterEach, describe, expect, it, vi } from "vitest";
import { renderApp } from "@nasiko/app-shell/testing/render-app";
import { server, setupMockServer } from "@nasiko/testing/pinned-seed";
import { clearDrafts } from "./drafts";
import { clearChatRegistry } from "./registry";

setupMockServer();

// Changed: the lab mocked its one-function `@/lib/download`; `@nasiko/ui/lib` holds every helper, so only
// `downloadText` is replaced.
vi.mock("@nasiko/ui/lib", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@nasiko/ui/lib")>()),
  downloadText: vi.fn(),
}));
const { downloadText } = await import("@nasiko/ui/lib");

const DONE = "5eedc000-0000-4000-8000-00000000c001";

afterEach(() => {
  clearChatRegistry();
  clearDrafts();
});

describe("export", () => {
  it("Export as Markdown downloads the loaded history under the chat title", async () => {
    renderApp(`/chat/${DONE}`);
    await screen.findByText("Three incidents");
    await userEvent.click(screen.getByRole("button", { name: "More chat actions" }));
    await userEvent.click(await screen.findByRole("menuitem", { name: "Export as Markdown" }));
    expect(downloadText).toHaveBeenCalledOnce();
    const [name, text, type] = vi.mocked(downloadText).mock.calls[0]!;
    expect(name).toMatch(/^[\w.-]+\.md$/);
    expect(text).toMatch(/^# Summarise last week’s incidents/);
    expect(text).toContain("**Three incidents**");
    expect(type).toBe("text/markdown;charset=utf-8");
  });
});

describe("rename", () => {
  it("Rename from the ⋯ menu focuses the title input (ISSUE-005)", async () => {
    renderApp(`/chat/${DONE}`);
    // Rename is enabled once the chat's row has loaded (its title shows).
    await screen.findByRole("button", { name: "Summarise last week’s incidents" });
    await userEvent.click(screen.getByRole("button", { name: "More chat actions" }));
    await userEvent.click(await screen.findByRole("menuitem", { name: "Rename" }));
    await waitFor(() => expect(screen.getByLabelText("Chat title")).toHaveFocus());
  });

  it("a rename the server refuses keeps the input open and says why", async () => {
    server.use(
      http.put("/api/chat/sessions/:id", () =>
        HttpResponse.json({ message: "title rejected" }, { status: 422 }),
      ),
    );
    renderApp(`/chat/${DONE}`);
    await userEvent.click(
      await screen.findByRole("button", { name: "Summarise last week’s incidents" }),
    );
    const input = screen.getByLabelText("Chat title");
    await userEvent.clear(input);
    await userEvent.type(input, "New name{Enter}");
    // Shown next to the input and spoken once by the page's only live region.
    expect(
      await screen.findByText("Server said: title rejected", { selector: ".text-destructive" }),
    ).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("Server said: title rejected");
    expect(screen.getByLabelText("Chat title")).toHaveValue("Summarise last week’s incidents");
  });

  it("a blur before the rename input ever had focus does not close or save it (ISSUE-005)", async () => {
    const puts: string[] = [];
    const onStart = ({ request }: { request: Request }) => {
      if (request.method === "PUT") puts.push(request.url);
    };
    server.events.on("request:start", onStart);
    renderApp(`/chat/${DONE}`);
    await userEvent.click(
      await screen.findByRole("button", { name: "Summarise last week’s incidents" }),
    );
    // Fire the blur synchronously, before the input's own focus frame runs.
    fireEvent.blur(screen.getByLabelText("Chat title"));
    expect(screen.getByLabelText("Chat title")).toBeInTheDocument();
    expect(puts).toHaveLength(0);
    server.events.removeListener("request:start", onStart);
  });
});

describe("View session (ISSUE-002)", () => {
  it("in mock mode opens the chat’s session, not the missing-session state", async () => {
    const { router } = renderApp(`/chat/${DONE}`);
    await userEvent.click(await screen.findByRole("link", { name: "View session" }));
    await waitFor(() => {
      expect(router.state.location.pathname).toBe(`/sessions/${DONE}`);
    });
    // Changed: the trace page (/sessions/[sessionId]) is not ported yet, so the test stops at the URL; the page's
    // "Session ·" header and its missing-session state return with the Sessions/trace batch.
  });
});
