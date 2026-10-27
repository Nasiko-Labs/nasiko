/**
 * "Forgot password?" on the sign-in page, in the desktop app only, while it runs Nasiko on this computer. That server
 * keeps the only copy of the password, so the way back is to start it over: the link opens the app's own reset page
 * (`/__nasiko/local/reset`, ui/desktop-tauri/src-tauri/src/server.rs), which confirms, deletes the server's data and
 * asks for a new admin account. A browser, or the app pointed at a hosted server, shows nothing (`localReset.ts`).
 */
import { Suspense, use } from 'react'
import { copy } from '@nasiko/app-core/shell/copy'
import { RESET_PAGE, resettable } from './localReset'

function ResetLine() {
  if (!use(resettable())) return null
  return (
    <p className="mt-4 text-center text-sm text-muted-foreground">
      {/* A full page load: the reset page is the desktop app's, not a route of this app. */}
      <a
        href={RESET_PAGE}
        className="font-medium text-foreground underline underline-offset-4 hover:text-muted-foreground"
      >
        {copy.forgotPassword.link}
      </a>{' '}
      {copy.forgotPassword.lead}
    </p>
  )
}

/** The sign-in page's line under the form; nothing outside the desktop app's local server, or while it asks. */
export function ForgotPasswordLink() {
  return (
    <Suspense fallback={null}>
      <ResetLine />
    </Suspense>
  )
}
