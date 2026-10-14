// Ported from nasiko-ui-lab@1b99bdda common/src/routes/_app/chat.index.tsx and chat.$sessionId.tsx. Local edits:
// route components (the SPA's generated route table) in place of `createFileRoute('/_app/chat/')` and `createFileRoute('/_app/chat/$sessionId')`:
// `validateSearch` is `useSearchState(chatSearchSchema)` (values arrive as strings, so `?auto=1` is "1" and
// `?agent=123` the text "123", both of which the schema already took from the lab's JSON parser), and the session
// id comes from `useParams()` (the `[sessionId]` segment, decoded as the lab's router decoded `$sessionId`); the two
// routes stay two components, as the lab's were (moving between /chat and a chat remounts the page in both); the
// loaders' `preload` prefetch is `prefetchChat(queryClient, scope, sessionId?)` (./api.ts), for a link's intent
// handler; `Suspense` around `useSearchParams` (kept from the Next-era port; harmless in the SPA); Prettier
// formatting.
"use client";

import { useParams } from "@nasiko/app-core/navigation";
import { Suspense } from "react";
import { useSearchState } from "@nasiko/app-core/url-state";
import { ChatPage } from "./ChatPage";
import { chatSearchSchema } from "./search";

/** A new chat: `?agent=` picks the agent (a UUID or a name, plan §6.11). */
export function ChatRoute() {
  return (
    <Suspense fallback={null}>
      <NewChat />
    </Suspense>
  );
}

/** One chat. A sibling of /chat (not nested), so the page owns its full-height layout. */
export function ChatSessionRoute() {
  return (
    <Suspense fallback={null}>
      <Session />
    </Suspense>
  );
}

function NewChat() {
  const { search } = useSearchState(chatSearchSchema);
  return <ChatPage search={search} />;
}

function Session() {
  const { sessionId } = useParams<{ sessionId: string }>();
  const { search } = useSearchState(chatSearchSchema);
  return <ChatPage sessionId={sessionId} search={search} />;
}
