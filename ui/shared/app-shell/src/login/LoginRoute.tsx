import * as z from "zod/mini";
import { useSearchParams } from "@nasiko/app-core/navigation";
import { LoginPage } from "./login-page";
import { loginSearchSchema } from "./search";
import { LoginProviders } from "./login-providers";

export function LoginRoute() {
  const search = z.parse(loginSearchSchema, Object.fromEntries(useSearchParams()));
  return (
    <LoginProviders>
      <LoginPage search={search} />
    </LoginProviders>
  );
}
