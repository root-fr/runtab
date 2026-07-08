import { Component, type ErrorInfo, type ReactNode } from "react";

interface Props {
  children: ReactNode;
}

interface State {
  error: Error | null;
}

// A last-resort guard so a single render throw (e.g. an unexpected null field)
// degrades to a readable message instead of unmounting the whole tree into a
// blank page.
export class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo): void {
    console.error("runtab dashboard error", error, info);
  }

  render(): ReactNode {
    const { error } = this.state;
    if (!error) return this.props.children;
    return (
      <div className="grid min-h-screen place-items-center bg-background px-4 text-center text-foreground">
        <div className="max-w-md space-y-3">
          <h1 className="text-lg font-semibold">Something went wrong rendering the dashboard.</h1>
          <p className="text-sm text-muted-foreground">
            Your local data is safe. This is only a display error. Reloading usually fixes it.
          </p>
          <p className="break-words text-xs text-muted-foreground">{error.message}</p>
          <button
            onClick={() => window.location.reload()}
            className="rounded-md border border-border px-3 py-1.5 text-sm text-foreground transition-colors hover:bg-secondary"
          >
            Reload
          </button>
        </div>
      </div>
    );
  }
}
