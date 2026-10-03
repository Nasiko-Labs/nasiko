/** The guide's public entry: the dialog loads in its own chunk when first shown (the shell budget has no room). */
import { deferred } from '@/app/deferred'

export const OnboardingDialog = deferred(() =>
  import('./OnboardingDialog').then((m) => m.OnboardingDialog),
)
