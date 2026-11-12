import { Link } from '@tanstack/react-router'
import { FieldDescription } from '@nasiko/ui/primitives/field'
import { copy } from '../copy'

/** Under a name fixed by a new-version deploy (`search.ts` `newVersion`): why, and the way out to a fresh deploy. */
export function LockedNameHint({ id }: { id: string }) {
  return (
    <FieldDescription id={id} className="text-xs">
      {copy.deploy.nameLocked}{' '}
      <Link to="/deploy" search={{}} className="text-primary-text underline underline-offset-2">
        {copy.deploy.deployOther}
      </Link>
    </FieldDescription>
  )
}
