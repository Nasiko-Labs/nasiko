/**
 * The rail's chats in the order shown (view, filter, date groups), for the header's Previous/Next. The rail writes it;
 * it outlives the rail (the phone sheet closes), and a sign-out's full page load clears it.
 */
import { create } from 'zustand'

export const useRailOrder = create<{ ids: readonly string[] }>(() => ({ ids: [] }))

export const setRailOrder = (ids: readonly string[]) => useRailOrder.setState({ ids })
