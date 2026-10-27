/** The assessment form's one write: `POST /api/assessments` (`oss/server/src/assessment.rs`), stored in S3. */
import { useMutation } from '@tanstack/react-query'
import { apiData } from '@nasiko/data-access/client'
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
