export const UI_EVENTS = {
  CONFIG_READY: 'config:ready',
  THEME_CHANGED: 'theme:changed',
  ROUTE_CHANGED: 'route:changed',
} as const;

type Listener<T> = (payload: T) => void;
type ListenerSet = Set<Listener<unknown>>;

const listeners = new Map<string, ListenerSet>();

export function on<T = unknown>(event: string, fn: Listener<T>): () => void {
  let set = listeners.get(event);
  if (!set) {
    set = new Set();
    listeners.set(event, set);
  }
  set.add(fn as Listener<unknown>);
  return () => set?.delete(fn as Listener<unknown>);
}

export function emit<T = unknown>(event: string, payload?: T): void {
  const set = listeners.get(event);
  if (!set) return;
  for (const fn of set) {
    try {
      (fn as Listener<T>)(payload as T);
    } catch (err) {
      console.error(`event ${event} listener threw`, err);
    }
  }
}
