import { describe, expect, it } from 'vitest'
import type { Skill } from '@nasiko/features/agents/types'
import { type AgentApp, appsToConnectIn, orderApps } from './agentApps'

const app = (name: string, label: string, status: AgentApp['status']): AgentApp => ({
  id: name,
  name,
  label,
  logoUrl: null,
  authType: null,
  status,
})
const skill = (tags: string[]): Skill => ({ id: 's', name: 's', description: '', tags })

describe('orderApps', () => {
  const apps = [
    app('zendesk', 'Zendesk', 'needs_connection'),
    app('slack', 'Slack', 'connected'),
    app('googledrive', 'Google Drive', 'needs_connection'),
    app('linear', 'Linear', 'connected'),
    app('private', 'Private', 'not_shared'),
  ]

  it('connected apps first, then the ones to connect, the card naming order within each; unreachable ones dropped', () => {
    const order = orderApps(apps, [skill(['google drive', 'linear', 'connected apps'])]).map(
      (a) => a.name,
    )
    // Slack is connected but not on the card: still ahead of Google Drive, which the card names but isn't connected.
    expect(order).toEqual(['linear', 'slack', 'googledrive', 'zendesk'])
  })

  it('without a card: connected first, then by label', () => {
    expect(orderApps(apps).map((a) => a.name)).toEqual([
      'linear',
      'slack',
      'googledrive',
      'zendesk',
    ])
  })

  it("keeps the card's own order among its apps, not the alphabet", () => {
    const many = [
      app('asana', 'Asana', 'needs_connection'),
      app('notion', 'Notion', 'needs_connection'),
      app('googledrive', 'Google Drive', 'needs_connection'),
    ]
    const order = orderApps(many, [
      skill(['meeting notes']),
      skill(['google drive', 'notion']),
      skill(['asana']),
    ])
    expect(order.map((a) => a.name)).toEqual(['googledrive', 'notion', 'asana'])
  })
})

describe('appsToConnectIn', () => {
  const apps = [
    app('gmail', 'Gmail', 'needs_connection'),
    app('slack', 'Slack', 'connected'),
    app('googledrive', 'Google Drive', 'needs_connection'),
    app('monday', 'Monday', 'needs_connection'),
  ]

  it('finds the unconnected apps a reply names, as whole words written as the app writes them', () => {
    const tip =
      '**Try it on your own Gmail:** send "Summarize my unread emails from today in Gmail".'
    expect(appsToConnectIn(tip, apps).map((a) => a.name)).toEqual(['gmail'])
    expect(appsToConnectIn('Saved to Google Drive.', apps).map((a) => a.name)).toEqual([
      'googledrive',
    ])
    expect(appsToConnectIn('saved to google drive', apps)).toEqual([])
  })

  it('ignores connected apps, partial words and replies that name none', () => {
    expect(appsToConnectIn('Posted to Slack #eng', apps)).toEqual([])
    expect(appsToConnectIn('Gmailer and Google Drives', apps)).toEqual([])
    expect(appsToConnectIn('Here is your summary.', apps)).toEqual([])
  })

  it("a weekday is not an app when the card doesn't name it (Monday, the meeting-notes reply)", () => {
    const reply = 'Lena: tell the waitlist on Monday. Try it on your own Google Drive.'
    const card = [skill(['meeting notes', 'google drive', 'gmail', 'connected apps'])]
    expect(appsToConnectIn(reply, apps, card).map((a) => a.name)).toEqual(['googledrive'])
    // A card naming none of the agent's apps (a user's own agent) restricts nothing.
    expect(appsToConnectIn(reply, apps, [skill(['research'])]).map((a) => a.name)).toEqual([
      'googledrive',
      'monday',
    ])
  })
})
