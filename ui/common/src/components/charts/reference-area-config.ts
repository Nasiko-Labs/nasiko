import { Children, isValidElement, type ReactElement, type ReactNode } from 'react'
import { normalizeYAxisId } from './y-axis-scales'

export interface ReferenceAreaConfig {
  yAxisId: string
  y1?: number
  y2?: number
  axisLabelColor?: string
}

interface ReferenceAreaConfigProps {
  yAxisId?: string | number
  y1?: number
  y2?: number
  axisLabelColor?: string
}

function getChildComponentName(child: ReactElement) {
  const childType = child.type as { displayName?: string; name?: string }
  return typeof child.type === 'function' ? childType.displayName || childType.name || '' : ''
}

function isReferenceAreaElement(child: ReactElement): boolean {
  return getChildComponentName(child) === 'ReferenceArea'
}

/** Collect {@link ReferenceArea} props from chart children for axis label styling. */
export function extractReferenceAreaConfigs(children: ReactNode): ReferenceAreaConfig[] {
  const configs: ReferenceAreaConfig[] = []

  const visit = (node: ReactNode) => {
    Children.forEach(node, (child) => {
      if (!isValidElement(child)) {
        return
      }

      if (isReferenceAreaElement(child)) {
        const props = child.props as ReferenceAreaConfigProps | undefined
        if (props) {
          configs.push({
            yAxisId: normalizeYAxisId(props.yAxisId),
            y1: props.y1,
            y2: props.y2,
            axisLabelColor: props.axisLabelColor,
          })
        }
        return
      }

      const childProps = child.props as { children?: ReactNode } | undefined
      if (childProps?.children) {
        visit(childProps.children)
      }
    })
  }

  visit(children)
  return configs
}
