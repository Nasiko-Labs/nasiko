/**
 * Account settings → Keyboard shortcuts: a read-only list of every key the app answers (`copy.shortcuts`), grouped by
 * where it works. One card per group; each row is the action (and where) on the left, its keys on the right. Keys
 * that are alternatives are separated by "or"; keys pressed together sit in one group.
 */
import { PageHeader } from '@nasiko/ui/composites/page-header'
import { Kbd, KbdGroup } from '@nasiko/ui/primitives/kbd'
import { IS_MAC } from '@nasiko/ui/lib/keys'
import { SettingHeading, SettingRow, SettingRows } from './components/SettingRow'
import { copy } from './copy'

const label = (key: string) => (key === 'Mod' ? (IS_MAC ? '⌘' : 'Ctrl') : key)

export function ShortcutsPage() {
  return (
    <div className="flex flex-col">
      <PageHeader title={copy.shortcuts.label} description={copy.shortcuts.sub} />
      {copy.shortcuts.groups.map((g) => (
        <section key={g.title}>
          <SettingHeading>{g.title}</SettingHeading>
          {g.hint ? <p className="-mt-2 pb-3 text-sm text-muted-foreground">{g.hint}</p> : null}
          <SettingRows>
            {g.rows.map((r) => (
              <SettingRow key={r.action + (r.where ?? '')} label={r.action} hint={r.where}>
                <span className="flex flex-wrap items-center gap-1.5 @[640px]:justify-end">
                  {r.keys.map((combo, i) => (
                    <span key={combo.join('+')} className="flex items-center gap-1.5">
                      {i > 0 ? <span className="text-xs text-muted-foreground">or</span> : null}
                      <KbdGroup>
                        {combo.map((k) => (
                          <Kbd key={k}>{label(k)}</Kbd>
                        ))}
                      </KbdGroup>
                    </span>
                  ))}
                </span>
              </SettingRow>
            ))}
          </SettingRows>
        </section>
      ))}
    </div>
  )
}
