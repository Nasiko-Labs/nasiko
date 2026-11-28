import { render, screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'
import { DesktopReport } from './DesktopReport'

const first = {
  id: 'a00001',
  task: 'confirm',
  harness: 'scripted',
  model: 'control',
  trial: 1,
  outcome: 'time_limit',
  resolved: true,
  seconds: 1.5,
  screenshots: ['https://example.test/frame.png'],
  router_usd: 0,
  usage_status: 'recorded',
}
function upload(body: unknown) {
  const file = new File([JSON.stringify(body)], 'report.json', { type: 'application/json' })
  return userEvent.upload(screen.getByLabelText('Report JSON'), file)
}
describe('Desktop report import', () => {
  it('keeps grade, outcome and cost distinct and only loads explicitly selected screenshots', async () => {
    render(<DesktopReport />)
    await upload({
      version: 1,
      run_id: 'smoke',
      tasks: [{ id: 'confirm', prompt: 'Confirm the ticket.' }],
      attempts: [
        first,
        {
          ...first,
          id: 'a00002',
          trial: 2,
          outcome: 'grader_error',
          resolved: null,
          router_usd: null,
          usage_status: 'unavailable',
          screenshots: [],
        },
      ],
    })
    const table = await screen.findByRole('table', {
      name: 'Desktop attempts for the selected task',
    })
    expect(within(table).getByText('Passed')).toBeInTheDocument()
    expect(within(table).getByText('time limit')).toBeInTheDocument()
    expect(within(table).getByText('$0.00')).toBeInTheDocument()
    expect(within(table).getByText('unavailable')).toBeInTheDocument()
    expect(screen.queryByRole('img')).toBeNull()
    expect(screen.getByText(/Screenshot unavailable/)).toBeInTheDocument()
    await userEvent.click(
      within(table).getByRole('button', { name: 'Show scripted, control, trial 2' }),
    )
    expect(screen.getByText('No screenshots recorded.')).toBeInTheDocument()
  })
  it('rejects an unknown report version and recovers on a valid report', async () => {
    render(<DesktopReport />)
    await upload({ version: 2 })
    expect(await screen.findByRole('alert')).toHaveTextContent('version 1')
    await upload({ version: 1, run_id: 'empty', tasks: [], attempts: [] })
    expect(await screen.findByText('No attempts for this task.')).toBeInTheDocument()
    expect(screen.queryByRole('alert')).toBeNull()
  })
})
