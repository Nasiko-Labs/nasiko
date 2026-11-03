/**
 * Deploy an agent (plans/feat-deploy.md §4; approved mockup: Deploy variant C). The methods are three cards on shadcn
 * `Tabs`, with the method in the URL (`?method=`, replaced, not pushed): Upload, GitHub, Registry.
 */
import { useQuery } from '@tanstack/react-query'
import { Container, FolderGit2, Upload } from 'lucide-react'
import { TopBar } from '@nasiko/app-core/shell/TopBar'
import { PageHeader } from '@nasiko/ui/composites/page-header'
import { Skeleton } from '@nasiko/ui/primitives/skeleton'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@nasiko/ui/primitives/tabs'
import { meQuery } from '@nasiko/data-access/auth'
import { copy } from './copy'
import { GithubTab } from './GithubTab'
import { RegistryTab } from './RegistryTab'
import { DEPLOY_METHODS, type DeployMethod, type DeploySearch } from './search'
import { UploadTab } from './UploadTab'

const ICON = { upload: Upload, github: FolderGit2, registry: Container } as const

export function DeployPage({
  search,
  setSearch,
}: {
  search: DeploySearch
  setSearch: (patch: Partial<DeploySearch>) => void
}) {
  const me = useQuery(meQuery)
  const method: DeployMethod = search.method ?? 'upload'
  return (
    <div className="@container mx-auto flex w-full max-w-page flex-col gap-4">
      <TopBar trail={[copy.deploy.title]} />
      <PageHeader title={copy.deploy.title} description={copy.deploy.description} />
      <Tabs
        value={method}
        onValueChange={(v) =>
          setSearch({ method: v === 'upload' ? undefined : (v as DeployMethod) })
        }
        className="gap-4"
      >
        {/* The method is the page's first choice, so each is a card that says what it takes. Still tabs: arrow
            keys move between them, and the method stays in the URL. */}
        <TabsList
          data-tour="deploy-methods"
          aria-label={copy.methods.label}
          className="grid w-full grid-cols-3 items-stretch gap-3 bg-transparent p-0 group-data-[orientation=horizontal]/tabs:h-auto @max-[640px]:gap-2"
        >
          {DEPLOY_METHODS.map((m) => {
            const Icon = ICON[m]
            return (
              <TabsTrigger
                key={m}
                value={m}
                aria-label={copy.methods[m]}
                aria-describedby={`deploy-method-${m}`}
                className="h-auto min-w-0 flex-col items-start justify-start gap-1 rounded-lg border-border bg-card p-3 text-left font-normal whitespace-normal text-foreground shadow-xs hover:border-ring/60 @max-[640px]:justify-center @max-[640px]:p-2 pointer-coarse:min-h-11 data-active:border-primary data-active:bg-card data-active:ring-1 data-active:ring-primary dark:data-active:border-primary dark:data-active:bg-card"
              >
                <span className="flex items-center gap-2 text-sm font-medium">
                  {/* Phones: words only, so the three cards fit side by side. */}
                  <Icon aria-hidden className="size-4 @max-[480px]:hidden" />
                  {copy.methods[m]}
                </span>
                <span
                  id={`deploy-method-${m}`}
                  className="text-xs text-muted-foreground @max-[640px]:sr-only"
                >
                  {copy.methodHints[m]}
                </span>
              </TabsTrigger>
            )
          })}
        </TabsList>
        <TabsContent value="upload">
          {me.data ? (
            <UploadTab
              userId={me.data.sub}
              prefill={{ name: search.name, version: search.version }}
            />
          ) : (
            <Skeleton className="h-80 w-full" />
          )}
        </TabsContent>
        <TabsContent value="github">
          <GithubTab search={search} setSearch={setSearch} />
        </TabsContent>
        <TabsContent value="registry">
          {me.data ? <RegistryTab userId={me.data.sub} /> : <Skeleton className="h-48 w-full" />}
        </TabsContent>
      </Tabs>
    </div>
  )
}
