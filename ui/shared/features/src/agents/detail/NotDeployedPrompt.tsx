/**
 * The deploy prompt for an agent that isn't deployed (plans/feat-deploy.md §7 entry points): Deploy an agent, or the
 * CLI. Overview shows it above the hub; Details shows it in place of the active version.
 */
import { DeployAgentButton } from '@nasiko/features/deploy/components/DeployAgentButton'
import { copy as deployCopy } from '@nasiko/features/deploy/copy'
import { CliCommand } from '../components/bits'
import { copy } from '../copy'
import { DEPLOY_CMD } from '../format'

export function NotDeployedPrompt({ name }: { name: string }) {
  return (
    <div className="flex flex-col items-start gap-2 text-sm">
      <p className="text-muted-foreground">{copy.notDeployed}.</p>
      <DeployAgentButton name={name} />
      <p className="text-xs text-muted-foreground">{deployCopy.entry.orCli}</p>
      <CliCommand command={DEPLOY_CMD} />
    </div>
  )
}
