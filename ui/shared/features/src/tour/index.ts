/** The tour's lazy part: the overlay and every tour's text load in their own chunk, the first time a tour opens. */
import { deferred } from '@nasiko/app-core/deferred'

export const TourOverlay = deferred(() => import('./TourOverlay').then((m) => m.TourOverlay))
