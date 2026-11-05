/** Whether the assessment banner (AssessmentCta.tsx) shows: dismissed for good in this browser by its X. */
import { useState } from 'react'

const DISMISSED_KEY = 'openruntime.assessmentDismissed'

function readDismissed(): boolean {
  try {
    return localStorage.getItem(DISMISSED_KEY) === '1'
  } catch {
    return false
  }
}

/** Whether the banner shows, and how to dismiss it. The shell needs the first to size the sticky header. */
export function useAssessmentBanner(): [shown: boolean, dismiss: () => void] {
  const [dismissed, setDismissed] = useState(readDismissed)
  const dismiss = () => {
    try {
      localStorage.setItem(DISMISSED_KEY, '1')
    } catch {
      // Storage blocked: hidden for this page load only.
    }
    setDismissed(true)
  }
  return [!dismissed, dismiss]
}
