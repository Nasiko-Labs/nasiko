/**
 * Deploy an agent (plans/feat-deploy.md §4; approved mockup: Deploy variant C). The method row is shadcn `Tabs` with
 * the method in the URL (`?method=`, replaced, not pushed): Upload, GitHub, Registry.
 */
import { useQuery } from '@tanstack/react-query'
import { Container, FolderGit2, Upload } from 'lucide-react'
import { PageHeader } from '@/components/shared/page-header'
import { Skeleton } from '@/components/ui/skeleton'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { meQuery } from '@/lib/api/auth'
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
      <PageHeader title={copy.deploy.title} description={copy.deploy.description} />
      <Tabs
        value={method}
        onValueChange={(v) =>
          setSearch({ method: v === 'upload' ? undefined : (v as DeployMethod) })
        }
        className="gap-4"
      >
        {/* The method row stays put while the form scrolls under it (Sessions' sticky bar). */}
        <div className="sticky top-0 z-20 -mx-4 bg-background/95 px-4 py-2 backdrop-blur">
          <TabsList
            aria-label={copy.methods.label}
            className="h-auto w-full justify-start @[768px]:w-fit"
          >
            {DEPLOY_METHODS.map((m) => {
              const Icon = ICON[m]
              return (
                <TabsTrigger
                  key={m}
                  value={m}
                  className="min-w-0 flex-1 gap-2 px-2 py-2 @[480px]:px-4 @[768px]:flex-none pointer-coarse:min-h-11"
                >
                  {/* Phones: words only, so the three tabs fit side by side without scrolling the page. */}
                  <Icon aria-hidden className="size-4 @max-[480px]:hidden" />
                  {copy.methods[m]}
                </TabsTrigger>
              )
            })}
          </TabsList>
        </div>
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
