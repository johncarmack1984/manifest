import { useEffect, useState } from "react";

interface AsyncState<T> {
  data?: T;
  error?: string;
  loading: boolean;
}

export function useAsync<T>(fn: () => Promise<T>, deps: unknown[]): AsyncState<T> {
  const [state, setState] = useState<AsyncState<T>>({ loading: true });
  useEffect(() => {
    let live = true;
    // Revalidate without dropping data: dep changes (e.g. the hourly token
    // renewal) re-run the fetch in the background while the page keeps
    // rendering the last result instead of blanking to a spinner.
    setState((s) => ({ ...s, error: undefined, loading: true }));
    fn()
      .then((data) => live && setState({ data, loading: false }))
      .catch((e) => live && setState({ error: String(e?.message ?? e), loading: false }));
    return () => {
      live = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, deps);
  return state;
}
