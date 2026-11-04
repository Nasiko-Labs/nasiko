import type { ReactNode } from 'react'
import { DialogDescription, DialogTitle } from '@nasiko/ui/primitives/dialog'

/** Each step's h2 is the dialog's title (Base UI names the dialog by it), its intro the description. The title can be
 *  marked up (Welcome's words arrive one by one); its text is still the dialog's name. */
export function StepHeading({ title, intro }: { title: ReactNode; intro: string }) {
  return (
    <div className="flex flex-col gap-3">
      <DialogTitle className="text-3xl leading-tight font-semibold tracking-tight text-balance md:text-4xl">
        {title}
      </DialogTitle>
      <DialogDescription className="max-w-2xl text-base text-pretty">{intro}</DialogDescription>
    </div>
  )
}

/** A titled half of a step that has two (Key and agent): an h3 under the step's own heading. */
export function SectionHeading({ title, intro }: { title: string; intro: string }) {
  return (
    <div className="flex flex-col gap-1">
      <h3 className="text-lg font-semibold">{title}</h3>
      <p className="text-sm text-pretty text-muted-foreground">{intro}</p>
    </div>
  )
}
