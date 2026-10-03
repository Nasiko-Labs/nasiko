/**
 * Delete a config (plan §4.4). The default's delete warns what it moves; a 409 "attached to N agent(s)" lists the
 * caller's attached agents (complete: an attach needs the owner's config) with Change routing, which closes this
 * dialog first so overlays never stack. A listed count that disagrees with the server's re-reads the rows.
 */
import { useQueryClient } from '@tanstack/react-query'
import { useEffect } from 'react'
import { Button } from '@/components/ui/button'
import { ConfirmDialog } from '@/features/agents/components/dialogs'
import { useAnnounce } from '../announce'
import { routerKeys, useDeleteConfig } from '../api'
import { copy } from '../copy'
import { inUseCount, routerError } from '../errors'
import { affects, countText, type RowRead } from '../routing'
import type { LlmConfig } from '../types'

export function DeleteConfigDialog({
  config,
  reads,
  attached,
  onClose,
  onChangeRouting,
}: {
  config: LlmConfig | null
  reads: readonly RowRead[]
  /** The caller's agents attached to this config, from the page's rows: [id, display name]. */
  attached: readonly (readonly [string, string])[]
  onClose: () => void
  onChangeRouting: (agentId: string) => void
}) {
  const del = useDeleteConfig()
  const qc = useQueryClient()
  const announce = useAnnounce()
  const error = del.error
  const serverCount = inUseCount(error)
  const inUse = serverCount !== null
  // R-L3: no reverse lookup, so the list is the page's own join; if it disagrees with the server, re-read it.
  useEffect(() => {
    if (serverCount !== null && serverCount !== attached.length)
      void qc.invalidateQueries({ queryKey: routerKeys.agents })
  }, [inUse, serverCount, attached.length, qc])

  if (!config) return null
  const close = () => {
    del.reset()
    onClose()
  }
  return (
    <ConfirmDialog
      open
      onOpenChange={(o) => {
        if (!o) close()
      }}
      title={copy.deleteTitle(config.name)}
      body={
        config.is_default
          ? copy.affectsDeleteDefault(countText(affects('delete-default', config, reads)))
          : copy.deleteBody
      }
      confirmLabel={copy.delete}
      destructive
      pending={del.isPending}
      confirmDisabled={inUse}
      onConfirm={() => {
        del.mutate(config, {
          onSuccess: () => {
            announce(copy.deleted(config.name))
            close()
          },
        })
      }}
    >
      {inUse ? (
        <div className="space-y-2 text-sm" role="alert">
          <p>{copy.deleteInUse(String(serverCount ?? attached.length))}</p>
          <ul className="space-y-1">
            {attached.map(([id, name]) => (
              <li key={id} className="flex items-center justify-between gap-2">
                <span>{name}</span>
                <Button
                  className="pointer-coarse:min-h-11"
                  size="sm"
                  variant="outline"
                  onClick={() => {
                    close()
                    onChangeRouting(id)
                  }}
                >
                  {copy.changeRouting}
                </Button>
              </li>
            ))}
          </ul>
        </div>
      ) : error ? (
        <p role="alert" className="text-sm text-destructive">
          {routerError(error).problem} {routerError(error).action}
        </p>
      ) : null}
    </ConfirmDialog>
  )
}
