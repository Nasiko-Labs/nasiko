/**
 * The OAuth "Resource parameter" select (Automatic / Always / Never), shared by the register dialog and the settings
 * tab. It maps to the connector's `oauth_send_resource` (null / true / false) through `sendResourceValue`.
 */
import { Controller, type Control, type FieldValues, type Path } from 'react-hook-form'
import { Field, FieldDescription, FieldLabel } from '@nasiko/ui/primitives/field'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@nasiko/ui/primitives/select'
import { copy, SEND_RESOURCE_OPTION } from '../copy'
import { SEND_RESOURCES, type SendResource } from '../logic'

export function SendResourceField<T extends FieldValues & { oauth_send_resource: SendResource }>({
  control,
  id,
}: {
  control: Control<T>
  id: string
}) {
  return (
    <Field className="gap-1.5">
      <FieldLabel htmlFor={id}>{copy.fields.oauth_send_resource.label}</FieldLabel>
      <Controller
        control={control}
        name={'oauth_send_resource' as Path<T>}
        render={({ field }) => (
          <Select value={field.value as SendResource} onValueChange={field.onChange}>
            <SelectTrigger id={id} className="w-full">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {SEND_RESOURCES.map((o) => (
                <SelectItem key={o} value={o}>
                  {SEND_RESOURCE_OPTION[o]}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        )}
      />
      <FieldDescription>{copy.fields.oauth_send_resource.hint}</FieldDescription>
    </Field>
  )
}
