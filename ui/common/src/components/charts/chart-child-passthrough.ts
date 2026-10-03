import { Children, Fragment, isValidElement, type ReactElement, type ReactNode } from 'react'

/** Marker on wrapper components whose single child should inherit clip classification. */
export const CHART_CLIP_PASSTHROUGH = '__chartClipPassthrough' as const

export function isChartClipPassthrough(type: unknown): boolean {
  return (
    typeof type === 'function' &&
    (type as { [CHART_CLIP_PASSTHROUGH]?: boolean })[CHART_CLIP_PASSTHROUGH] === true
  )
}

/** Unwrap visibility wrappers so `Grid` / axes stay outside the series clip. */
export function resolveChartChildElement(child: ReactElement): ReactElement {
  if (isChartClipPassthrough(child.type)) {
    const inner = (child.props as { children?: unknown }).children
    if (isValidElement(inner)) {
      return resolveChartChildElement(inner)
    }
  }
  return child
}

/** Walk chart children, flattening React fragments (studio often groups layers in `<>...</>`). */
export function forEachChartChild(
  children: ReactNode,
  callback: (child: ReactElement, index: number) => void,
) {
  let index = 0
  const visit = (nodes: ReactNode) => {
    Children.forEach(nodes, (child) => {
      if (!isValidElement(child)) {
        return
      }
      if (child.type === Fragment) {
        visit((child.props as { children?: ReactNode }).children)
        return
      }
      callback(child, index)
      index += 1
    })
  }
  visit(children)
}

const CLIP_EXCLUDED_COMPONENT_NAMES = new Set([
  'Background',
  'Grid',
  'XAxis',
  'YAxis',
  'BarXAxis',
  'BarYAxis',
  'LiveXAxis',
  'LiveYAxis',
])

const UNDERLAY_COMPONENT_NAMES = new Set(['ReferenceArea', 'BarColumnTrack'])

/** Markers render after the interaction overlay so they stay clickable. */
export function isPostOverlayComponent(child: ReactElement): boolean {
  const childType = child.type as {
    displayName?: string
    name?: string
    __isChartMarkers?: boolean
    __isPostOverlay?: boolean
  }

  if (childType.__isChartMarkers || childType.__isPostOverlay) {
    return true
  }

  const componentName =
    typeof child.type === 'function' ? childType.displayName || childType.name || '' : ''

  return (
    componentName === 'ChartMarkers' ||
    componentName === 'MarkerGroup' ||
    componentName === 'ChartBrush'
  )
}

/** Renders above grid/axes but below series; excluded from grow-clip reveal. */
export function isUnderlayComponent(child: ReactElement): boolean {
  const childType = child.type as { displayName?: string; name?: string }
  const componentName =
    typeof child.type === 'function' ? childType.displayName || childType.name || '' : ''
  return UNDERLAY_COMPONENT_NAMES.has(componentName)
}

/** Grid and axes stay visible during series clip reveal (e.g. loading → ready). */
export function isClipExcludedComponent(child: ReactElement): boolean {
  const childType = child.type as { displayName?: string; name?: string }
  const componentName =
    typeof child.type === 'function' ? childType.displayName || childType.name || '' : ''
  return CLIP_EXCLUDED_COMPONENT_NAMES.has(componentName)
}

/**
 * SVG layer lists from chart shells need stable keys when rendered as arrays. Lab: `Children.toArray` keeps a
 * child's own key and gives the rest their position, as the kit's index fallback did.
 */
export function renderKeyedChartLayers(children: ReactElement[]) {
  return Children.toArray(children)
}
