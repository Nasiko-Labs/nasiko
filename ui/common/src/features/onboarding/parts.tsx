import { DialogDescription, DialogTitle } from '@/components/ui/dialog'

/** Each step's h2 is the dialog's title (Radix names the dialog by it), its intro the description. */
export function StepHeading({ title, intro }: { title: string; intro: string }) {
  return (
    <div className="flex flex-col gap-3">
      <DialogTitle className="text-3xl leading-tight font-semibold tracking-tight text-balance md:text-4xl">
        {title}
      </DialogTitle>
      <DialogDescription className="max-w-2xl text-base text-pretty">{intro}</DialogDescription>
    </div>
  )
}
