/**
 * Mounted by the Overview route: opens the guide for a first-time user and renders it while open. Leaving `/` closes
 * it, so a sign-out or a link never leaves a guide open for whoever comes next.
 */
import { useEffect } from 'react'
import { closeGuide, openGuide, useGuide, useGuideState } from './api'
import { OnboardingDialog } from './index'

export function GuideHost() {
  const guide = useGuide()
  const state = useGuideState()
  const { due, skip } = guide
  useEffect(() => {
    if (due && !state.open) openGuide('welcome', true)
  }, [due, state.open])
  useEffect(() => () => closeGuide(), [])
  if (!state.open) return null
  return (
    <OnboardingDialog
      // A new opening starts fresh (the Setup guide may open it at another step).
      key={state.step}
      initialStep={state.step}
      onClose={() => {
        // Closed before a persona was saved: hide it for this session (the server has no "skipped", O-B1).
        if (due) skip()
        closeGuide()
      }}
    />
  )
}
