/** The assessment form's two writes. The first: `POST /api/assessments` (`oss/server/src/assessment.rs`), stored in Neon by the assessment intake (`scripts/sales-intake/api/assessments.js`), which also sends it to Clay. */
import { useMutation } from '@tanstack/react-query'
import { apiData } from '@nasiko/data-access/client'
import type { CalendlyBooking } from '@nasiko/ui/composites/calendly-embed'
import type { Answers } from './AssessmentPage'

/** The answers on the wire: snake_case, and a company only from a team. */
export function wireOf(a: Answers) {
  return {
    applying_as: a.applyingAs,
    name: a.name.trim(),
    email: a.email.trim(),
    company: a.applyingAs === 'team' ? a.company.trim() : null,
    agents: a.agents,
    spend: a.spend,
    challenges: a.challenges,
    notes: a.notes.trim(),
  }
}

export function useSubmitAssessment() {
  return useMutation({
    mutationFn: (a: Answers) =>
      apiData<{ id: string }>('/api/assessments', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(wireOf(a)),
      }),
  })
}

/**
 * The second: the Calendly booking made on the last step, `POST /api/assessments/{id}/booking`
 * (`oss/server/src/assessment.rs`), which marks the saved assessment booked and sends it to Clay.
 */
export function useReportAssessmentBooking() {
  return useMutation({
    mutationFn: ({ id, booking }: { id: string; booking: CalendlyBooking }) =>
      apiData<{ id: string }>(`/api/assessments/${encodeURIComponent(id)}/booking`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(booking),
      }),
    // Calendly has the booking and has sent the invite either way; the assessment just isn't marked booked.
    onError: (e) => console.error('[assessment] booking not recorded', e),
  })
}
