/**
 * What each kind of media renders as once it is sent (§7.7): a picture shows, sound and video get
 * a player, everything else is a download chip — and every one of them is reachable as a file.
 */
import { render, screen, within } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import { apiUrl } from '@nasiko/data-access/client'
import { MessageAttachments } from './components/Attachments'
import type { FilePart } from './types'

// The API base (the page origin in tests): file URLs are absolute so they reach the API host.
const ORIGIN = apiUrl('/').origin

const part = (name: string, mime: string): FilePart => ({ id: `f-${name}`, name, size: 2048, mime })

const show = (files: FilePart[], pending = false) =>
  render(<MessageAttachments files={files} pending={pending} />)

describe('a sent message, by kind of media', () => {
  it('shows an image as a picture, labelled with its name', () => {
    show([part('photo.png', 'image/png')])
    expect(screen.getByRole('img', { name: 'photo.png' })).toHaveAttribute(
      'src',
      `${ORIGIN}/api/chat/files/f-photo.png/download`,
    )
  })

  it('gives sound and video a player, not a thumbnail', () => {
    const { container } = show([part('note.m4a', 'audio/mp4'), part('clip.mp4', 'video/mp4')])
    const audio = container.querySelector('audio')
    const video = container.querySelector('video')
    expect(audio).toHaveAttribute('controls')
    expect(video).toHaveAttribute('controls')
    expect(audio).toHaveAttribute('src', `${ORIGIN}/api/chat/files/f-note.m4a/download`)
    expect(video).toHaveAttribute('src', `${ORIGIN}/api/chat/files/f-clip.mp4/download`)
  })

  it('leaves a document or an unknown type as a download chip', () => {
    show([part('report.pdf', 'application/pdf'), part('mystery.dat', '')])
    for (const name of ['report.pdf', 'mystery.dat']) {
      const link = screen.getByText(name).closest('a')
      expect(link).toHaveAttribute('href', `${ORIGIN}/api/chat/files/f-${name}/download`)
      expect(link).toHaveAttribute('download')
    }
  })

  it('keeps a download beside every player, for a format this browser will not play', () => {
    const block = show([part('clip.mov', 'video/quicktime')]).container
    expect(within(block).getByRole('link')).toHaveAttribute(
      'href',
      `${ORIGIN}/api/chat/files/f-clip.mov/download`,
    )
  })

  it('links nothing while the upload is still in flight: those ids name no file yet', () => {
    const { container } = show([part('photo.png', 'image/png')], true)
    expect(container.querySelector('a')).toBeNull()
    expect(container.querySelector('img')).toBeNull()
    expect(screen.getByText('Uploading attachments…')).toBeInTheDocument()
    // The name is still shown: the bubble says what is being sent.
    expect(screen.getByText('photo.png')).toBeInTheDocument()
  })
})
