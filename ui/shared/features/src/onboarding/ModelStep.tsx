// Ported from nasiko-ui-lab@1b99bdda common/src/features/onboarding/ModelStep.tsx. Local edits: `"use client"`; the
// lab's `ApiError` is `ProductApiError`; Base UI's `RadioGroup` passes its value untyped (read as a string) and marks
// the checked item `data-checked` (`has-[[data-state=checked]]` -> `has-[[data-checked]]`); Prettier formatting.
"use client";

/**
 * Step 3: pick a built-in provider and save its key as the caller's default LLM config, through the router's own
 * `useSaveConfig` (key hygiene as there: the mutation keeps nothing and resets on unmount; the key is never read back).
 */
import { CircleCheck, KeyRound, Lock, TriangleAlert } from "lucide-react";
import { useEffect, useId, useState } from "react";
import { PanelError } from "@nasiko/ui/composites";
import { Alert, AlertDescription } from "@nasiko/ui/primitives";
import { Button } from "@nasiko/ui/primitives";
import { Field, FieldLabel } from "@nasiko/ui/primitives";
import { InputGroup, InputGroupAddon, InputGroupInput } from "@nasiko/ui/primitives";
import { RadioGroup, RadioGroupItem } from "@nasiko/ui/primitives";
import { Skeleton } from "@nasiko/ui/primitives";
import { Spinner } from "@nasiko/ui/primitives";
import { useCatalog, useSaveConfig } from "../router/api";
import { defaultSecretName } from "../router/routing";
import { ProductApiError } from "@nasiko/data-access/product";
import { copy } from "./copy";
import { guideProviders, providerName } from "./logic";
import { StepHeading } from "./parts";

export function ModelStep({
  connected,
  onConnected,
}: {
  /** What was connected ("Anthropic · claude-…"), shown on return to this step. */
  connected: string | null;
  onConnected: (summary: string) => void;
}) {
  const ids = useId();
  const catalog = useCatalog();
  const save = useSaveConfig();
  const { reset } = save;
  useEffect(
    () => () => {
      reset();
    },
    [reset],
  );
  const providers = guideProviders(catalog.data);
  const [picked, setPicked] = useState<string | null>(null);
  const provider = picked ?? providers[0]?.provider ?? null;
  const [key, setKey] = useState("");
  const [empty, setEmpty] = useState(false);

  const connect = () => {
    if (!provider) return;
    if (!key.trim()) {
      setEmpty(true);
      return;
    }
    // The server needs a model (or tier models) on every config: the provider's first catalog model, changeable later
    // in the LLM router.
    const model = providers.find((p) => p.provider === provider)?.models[0] ?? null;
    save.mutate(
      {
        mode: "create",
        body: {
          name: `${provider}-setup`,
          provider,
          model,
          api_key_secret_name: defaultSecretName(provider),
          secret_value: key.trim(),
          is_default: true,
        },
      },
      {
        onSuccess: () => {
          setKey("");
          onConnected(model ? `${providerName(provider)} · ${model}` : providerName(provider));
        },
      },
    );
  };

  return (
    <div className="flex flex-col gap-6">
      <StepHeading title={copy.model.title} intro={copy.model.intro} />
      {catalog.isPending ? (
        <Skeleton className="h-24 w-full" />
      ) : catalog.isError ? (
        <PanelError
          error={catalog.error}
          onRetry={() => void catalog.refetch()}
          what={copy.model.loadFailed}
        />
      ) : !providers.length ? (
        <p className="text-sm text-muted-foreground">{copy.model.noProviders}</p>
      ) : (
        <>
          <RadioGroup
            aria-label={copy.model.label}
            value={provider ?? ""}
            onValueChange={(v) => {
              setPicked(v as string);
              save.reset();
            }}
            className="grid gap-3 sm:grid-cols-3"
          >
            {providers.map((p) => (
              <label
                key={p.provider}
                htmlFor={`${ids}-${p.provider}`}
                className="flex cursor-pointer items-center gap-3 rounded-xl border bg-card p-4 hover:bg-accent/50 has-[[data-checked]]:border-foreground/50 has-[[data-checked]]:bg-accent"
              >
                <RadioGroupItem id={`${ids}-${p.provider}`} value={p.provider} />
                <span className="min-w-0">
                  <span className="block text-sm font-medium">{providerName(p.provider)}</span>
                  <span className="block text-xs text-muted-foreground">
                    {copy.model.models(p.models.length)}
                  </span>
                </span>
              </label>
            ))}
          </RadioGroup>
          {provider ? (
            <Field className="gap-2">
              <FieldLabel htmlFor={`${ids}-key`}>
                {copy.model.key(providerName(provider))}
              </FieldLabel>
              <div className="flex gap-2">
                <InputGroup className="h-10">
                  <InputGroupAddon>
                    <KeyRound aria-hidden />
                  </InputGroupAddon>
                  <InputGroupInput
                    id={`${ids}-key`}
                    type="password"
                    placeholder={copy.model.placeholder[provider] ?? copy.model.placeholderOther}
                    autoComplete="off"
                    spellCheck={false}
                    value={key}
                    onChange={(e) => {
                      setKey(e.target.value);
                      setEmpty(false);
                    }}
                  />
                </InputGroup>
                <Button
                  type="button"
                  variant="outline"
                  className="h-10"
                  disabled={save.isPending}
                  onClick={connect}
                >
                  {save.isPending ? <Spinner aria-hidden /> : null}
                  {save.isPending ? copy.model.connecting : copy.model.connect}
                </Button>
              </div>
            </Field>
          ) : null}
          {connected ? (
            <Alert role="status">
              <CircleCheck aria-hidden />
              <AlertDescription>{copy.model.connected(connected)}</AlertDescription>
            </Alert>
          ) : null}
          {empty ? (
            <p role="status" className="flex items-center gap-2 text-sm text-muted-foreground">
              <TriangleAlert aria-hidden className="size-4" />
              {copy.model.keyEmpty}
            </p>
          ) : null}
          {save.isError ? (
            <Alert variant="destructive">
              <TriangleAlert aria-hidden />
              <AlertDescription>
                {copy.model.saveFailed(
                  save.error instanceof ProductApiError &&
                    typeof save.error.body === "string" &&
                    save.error.body
                    ? save.error.body
                    : save.error.message,
                )}
              </AlertDescription>
            </Alert>
          ) : null}
          <p className="flex items-center gap-2 text-xs text-muted-foreground">
            <Lock aria-hidden className="size-3.5" />
            {copy.model.vault}
          </p>
        </>
      )}
    </div>
  );
}
