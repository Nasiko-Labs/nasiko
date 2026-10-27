/**
 * The village's own chunk (its drawings and motion), loaded on demand. One import, so the canvas and the Overview's
 * early starts (the view chosen, or a pointer on its switch) share a single download.
 */
export const loadVillage = () => import('./VillageCanvas')
