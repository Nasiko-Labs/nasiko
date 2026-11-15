import { markApiBaseUnresolved, setApiBase } from '@nasiko/data-access/client'
import { afterEach, describe, expect, it } from 'vitest'
import { connectCommand, controlPlaneUrl, serverUrl } from './commands'

afterEach(() => setApiBase(''))

describe('CLI commands that name this server (NAS-1464)', () => {
  it('names the page origin, never a hosted control plane', () => {
    expect(serverUrl()).toBe(window.location.origin)
    expect(serverUrl()).not.toMatch(/nasiko\.dev/)
  })

  it('registers this server and signs in to it in one command', () => {
    expect(connectCommand()).toBe(`nasiko connect ${window.location.origin}`)
  })
})

describe('CLI commands where the page is not the control plane (NAS-1597)', () => {
  it('names the workspace the page calls, not the page', () => {
    setApiBase('https://acme.nasiko.example')
    expect(serverUrl()).toBe('https://acme.nasiko.example')
    expect(connectCommand()).toBe('nasiko connect https://acme.nasiko.example')
  })

  it('names only the origin when the API base carries a path', () => {
    setApiBase('https://acme.nasiko.example/some/path')
    expect(serverUrl()).toBe('https://acme.nasiko.example')
  })
})

describe('CLI commands when the API base could not be looked up', () => {
  it('names no host rather than guess, until a base is set', () => {
    markApiBaseUnresolved()
    expect(serverUrl()).toBe('https://<your-cluster>')
    expect(connectCommand()).toBe('nasiko connect https://<your-cluster>')
    setApiBase('https://acme.app.nasiko.com')
    expect(serverUrl()).toBe('https://acme.app.nasiko.com')
  })
})

describe('Free workspace fallback on the management plane', () => {
  const page = (url: string) => new URL(url)

  it('prefixes free. to the management host while no workspace is set', () => {
    expect(
      controlPlaneUrl(page('https://app.nasiko.com/login'), page('https://app.nasiko.com/')),
    ).toBe('https://free.app.nasiko.com')
  })

  it('prefers the workspace the page calls once one is set', () => {
    expect(
      controlPlaneUrl(page('https://app.nasiko.com/'), page('https://acme.app.nasiko.com/')),
    ).toBe('https://acme.app.nasiko.com')
  })

  it('leaves any other host, self-hosted included, as it is', () => {
    expect(controlPlaneUrl(page('http://localhost:8080/'), page('http://localhost:8080/'))).toBe(
      'http://localhost:8080',
    )
    expect(
      controlPlaneUrl(page('https://nasiko.acme.com/'), page('https://nasiko.acme.com/')),
    ).toBe('https://nasiko.acme.com')
  })
})
